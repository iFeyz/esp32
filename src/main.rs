
use std::time::Duration;

use embedded_svc::wifi::{Configuration, AuthMethod};
use esp_idf_svc::wifi::AsyncWifi;
use esp_idf_svc::wifi::EspWifi;
use log::info;
use esp_idf_hal::peripheral::Peripheral;
use esp_idf_svc::timer::{EspTimerService, Task};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::ping::EspPing;
use anyhow::Result;
use esp_idf_hal::{prelude::Peripherals};
use esp_idf_svc::{timer::EspTaskTimerService, nvs::EspDefaultNvsPartition};
use esp_idf_svc::nvs::EspNvsPartition;
use esp_idf_svc::nvs::NvsDefault;
use embedded_svc::wifi::{ClientConfiguration};
use esp_idf_svc::{http::server::EspHttpServer};
use std::sync::{Arc, Mutex};
use esp_idf_hal::gpio::PinDriver;
use embedded_svc::{ http::Method::Post, io::Read};
use esp_idf_hal::units::*;
use esp_idf_hal::{ledc::{LedcTimerDriver, config::TimerConfig, LedcDriver}};



#[derive(Debug, Clone)]
struct Color {
    r: u8,
    g: u8,
    b: u8,
}

impl TryFrom<&str> for Color {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Ok(Color {
            r: u8::from_str_radix(value.get(0..2).unwrap(), 16)?,
            g: u8::from_str_radix(value.get(2..4).unwrap(), 16)?,
            b: u8::from_str_radix(value.get(4..6).unwrap(), 16)?,
        })
    }
}


fn main() {
    // It is necessary to call this function once. Otherwise some patches to the runtime
    // implemented by esp-idf-sys might not link properly. See https://github.com/esp-rs/esp-idf-template/issues/71
    esp_idf_svc::sys::link_patches();

    // Bind the log crate to the ESP Logging facilities
    esp_idf_svc::log::EspLogger::initialize_default();

    log::info!("Hello, world!");

    let peripherals = Peripherals::take().unwrap();
    let sysloop = EspSystemEventLoop::take().unwrap();
    let timer_service = EspTaskTimerService::new().unwrap();

    let _wifi = wifi(peripherals.modem, sysloop,Some(EspDefaultNvsPartition::take().unwrap()),timer_service).unwrap();


    let mut server = EspHttpServer::new(&Default::default()).unwrap();

    let led_timer = peripherals.ledc.timer0;
    let led_timer_driver = LedcTimerDriver::new(led_timer, &TimerConfig::new().frequency(1000.Hz())).unwrap();

    let red_channel = Arc::new(Mutex::new(LedcDriver::new(peripherals.ledc.channel0, &led_timer_driver, peripherals.pins.gpio3).unwrap()));
    let green_channel = Arc::new(Mutex::new(LedcDriver::new(peripherals.ledc.channel1, &led_timer_driver, peripherals.pins.gpio4).unwrap()));
    let blue_channel = Arc::new(Mutex::new(LedcDriver::new(peripherals.ledc.channel2, &led_timer_driver, peripherals.pins.gpio5).unwrap()));
    // Create esp pin handler 
    //let mut gpio1_pin = PinDriver::output(peripherals.pins.gpio1).unwrap();

    server.fn_handler("/", embedded_svc::http::Method::Get,move |mut req| {
        let mut response = req.into_ok_response().unwrap();
        response.write("Hello from ESP32-C3".as_bytes()).unwrap();
        //led_pin.lock().unwrap().toggle().unwrap();
        Ok::<_, anyhow::Error>(())
    }).unwrap();

    server.fn_handler("/color", embedded_svc::http::Method::Post,move |mut req| {
        let mut buffer = [0_u8;6];
        req.read_exact(&mut buffer)?;
        let color: Color = std::str::from_utf8(&buffer)?.try_into()?;
        println!("Color: {:?}", color);
        let mut response = req.into_ok_response()?;
        response.write("Color set".as_bytes())?;
        red_channel.lock().unwrap().set_duty(color.r as u32).unwrap();
        green_channel.lock().unwrap().set_duty(color.g as u32).unwrap();
        blue_channel.lock().unwrap().set_duty(color.b as u32).unwrap();
        Ok::<_, anyhow::Error>(())
    }).unwrap();

    // create the HTTP server loop
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }

}


pub fn wifi(
    modem: impl Peripheral<P = esp_idf_hal::modem::Modem> + 'static,
    sysloop: EspSystemEventLoop,
    nvs: Option<EspNvsPartition<NvsDefault>>,
    timer_service: EspTimerService<Task>,
) -> Result<AsyncWifi<EspWifi<'static>>> {
    use futures::executor::block_on;

    let mut wifi = AsyncWifi::wrap(
        EspWifi::new(modem, sysloop.clone(), nvs)?,
        sysloop,
        timer_service.clone(),
    )?;

    block_on(connect_wifi(&mut wifi))?;

    let ip_info = wifi.wifi().sta_netif().get_ip_info()?;

    println!("Wifi DHCP info: {:?}", ip_info);
    
    EspPing::default().ping(ip_info.subnet.gateway, &esp_idf_svc::ping::Configuration::default())?;
    Ok(wifi)

}

async fn connect_wifi(wifi: &mut AsyncWifi<EspWifi<'static>>) -> anyhow::Result<()> {

    const SSID: &str = "Wokwi-GUEST";
    const PASS: &str = "";

    let wifi_configuration: Configuration = Configuration::Client(ClientConfiguration {
        ssid: SSID.try_into().unwrap(),
        bssid: None,
        auth_method: AuthMethod::None, // Real AuthMethod will be WPA2Personal
        password: PASS.try_into().unwrap(),
        channel: None,
        ..Default::default()
    });

    info!("Wifi configuration: {:?}", wifi_configuration);

    wifi.set_configuration(&wifi_configuration)?;

    wifi.start().await?;
    info!("Wifi started");

    wifi.connect().await?;
    info!("Wifi connected");

    wifi.wait_netif_up().await?;
    info!("Wifi netif up");

    Ok(())
}