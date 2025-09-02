mod scan;

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

use crate::scan::{scan_wifi_with_resources, scan_networks_continuously};



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

    esp_idf_svc::log::EspLogger::initialize_default();

    log::info!("Starting dual-service ESP32 application...");

    log::info!("Taking peripherals in main...");
    let peripherals = Peripherals::take().unwrap();
    log::info!("Peripherals taken successfully");
    
    log::info!("Taking NVS partition in main...");
    let nvs = EspDefaultNvsPartition::take().unwrap();
    log::info!("NVS partition taken successfully");
    
    log::info!("Taking system event loop in main...");
    let sys_loop = EspSystemEventLoop::take().unwrap();
    log::info!("System event loop taken successfully");
    
    let timer_service = EspTaskTimerService::new().unwrap();

    log::info!("Setting up LED hardware...");
    let led_timer = peripherals.ledc.timer0;
    let led_timer_driver = LedcTimerDriver::new(led_timer, &TimerConfig::new().frequency(1000.Hz())).unwrap();
    
    let red_channel = Arc::new(Mutex::new(LedcDriver::new(peripherals.ledc.channel0, &led_timer_driver, peripherals.pins.gpio3).unwrap()));
    let green_channel = Arc::new(Mutex::new(LedcDriver::new(peripherals.ledc.channel1, &led_timer_driver, peripherals.pins.gpio4).unwrap()));
    let blue_channel = Arc::new(Mutex::new(LedcDriver::new(peripherals.ledc.channel2, &led_timer_driver, peripherals.pins.gpio5).unwrap()));

    log::info!("Setting up WiFi connection for API...");
    let _wifi_for_api = wifi(peripherals.modem, sys_loop.clone(), Some(nvs), timer_service).unwrap();

    let sys_loop_clone = sys_loop.clone();
    let red_channel_scanner = red_channel.clone();
    let green_channel_scanner = green_channel.clone();
    
    let _scanner_thread = std::thread::spawn(move || {
        log::info!("Starting WiFi scanner thread...");
        scan_networks_continuously(sys_loop_clone, red_channel_scanner, green_channel_scanner);
    });

    log::info!("Setting up HTTP server...");
    let mut server = EspHttpServer::new(&Default::default()).unwrap();

    server.fn_handler("/", embedded_svc::http::Method::Get, |req| {
        let mut response = req.into_ok_response().unwrap();
        let html = r#"
<!DOCTYPE html>
<html>
<head><title>ESP32-C3 WiFi Scanner & LED Controller</title></head>
<body>
    <h1>ESP32-C3 Services</h1>
    <p>WiFi Scanner running in background thread</p>
    <p>HTTP API ready with LED control</p>
    <p><a href="/status">Status</a></p>
    <p>POST to /color with 6-byte hex color (e.g., FF0000 for red)</p>
</body>
</html>
        "#;
        response.write(html.as_bytes()).unwrap();
        Ok::<_, anyhow::Error>(())
    }).unwrap();

    server.fn_handler("/status", embedded_svc::http::Method::Get, |req| {
        let mut response = req.into_ok_response().unwrap();
        let status = "WiFi Scanner: Active\nHTTP API: Active\nLED Controller: Ready";
        response.write(status.as_bytes()).unwrap();
        Ok::<_, anyhow::Error>(())
    }).unwrap();

    server.fn_handler("/color", embedded_svc::http::Method::Post, move |mut req| {
        let mut buffer = [0_u8; 6];
        req.read_exact(&mut buffer)?;
        let color: Color = std::str::from_utf8(&buffer)?.try_into()?;
        log::info!("Setting color: {:?}", color);
        
        let mut response = req.into_ok_response()?;
        response.write("Color set successfully".as_bytes())?;
        
        red_channel.lock().unwrap().set_duty(color.r as u32).unwrap();
        green_channel.lock().unwrap().set_duty(color.g as u32).unwrap();
        blue_channel.lock().unwrap().set_duty(color.b as u32).unwrap();
        
        Ok::<_, anyhow::Error>(())
    }).unwrap();

    log::info!("HTTP server started successfully");
    log::info!("Both WiFi scanner and HTTP API are now running in parallel");

    loop {
        std::thread::sleep(Duration::from_secs(5));
        log::info!("Main thread alive - services running");
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