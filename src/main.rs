
use std::time::Duration;

use embedded_svc::wifi::{Configuration as WifiConfiguration, AuthMethod};
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
use embedded_svc::{io::Read};
use esp_idf_hal::units::*;
use esp_idf_hal::{ledc::{LedcTimerDriver, config::TimerConfig, LedcDriver}};
use esp_idf_hal::spi::{SpiDeviceDriver, SpiDriver, config::Config as SpiConfig};
use esp_idf_hal::delay::FreeRtos;
use nrf24_rs::{Nrf24l01, config::{NrfConfig, PALevel}, SPI_MODE};
// MODE_0 is included in SPI_MODE from nrf24_rs
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};



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

fn generate_random_noise(length: usize) -> Vec<u8> {
    let mut noise = Vec::with_capacity(length);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    
    // Simple PRNG based on timestamp
    let mut seed = timestamp;
    for _ in 0..length {
        seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
        noise.push((seed >> 16) as u8);
    }
    noise
}

fn setup_nrf24l01_and_send_noise(
    spi2: esp_idf_hal::spi::SPI2,
    sclk: esp_idf_hal::gpio::Gpio6,
    mosi: esp_idf_hal::gpio::Gpio7,
    miso: esp_idf_hal::gpio::Gpio2,
    cs: esp_idf_hal::gpio::Gpio10,
    ce: esp_idf_hal::gpio::Gpio9,
) -> Result<()> {
    thread::spawn(move || {
        info!("Starting NRF24L01 setup...");
        info!("Pin configuration:");
        info!("  SCLK: GPIO6");
        info!("  MOSI: GPIO7"); 
        info!("  MISO: GPIO2");
        info!("  CS:   GPIO10");
        info!("  CE:   GPIO9");
        
        let spi_config = SpiConfig::new()
            .baudrate(500.kHz().into())  // Lower speed for stability
            .data_mode(SPI_MODE);
        
        info!("Initializing SPI with 500kHz baudrate");

        let spi = SpiDriver::new(
            spi2,
            sclk,
            mosi,
            Some(miso),
            &esp_idf_hal::spi::config::DriverConfig::new(),
        );
        
        let spi = match spi {
            Ok(s) => s,
            Err(e) => {
                log::error!("Failed to initialize SPI: {:?}", e);
                return;
            }
        };

        let spi_device = SpiDeviceDriver::new(&spi, Some(cs), &spi_config);
        let spi_device = match spi_device {
            Ok(d) => d,
            Err(e) => {
                log::error!("Failed to create SPI device: {:?}", e);
                return;
            }
        };
        
        let ce_pin = match PinDriver::output(ce) {
            Ok(p) => p,
            Err(e) => {
                log::error!("Failed to configure CE pin: {:?}", e);
                return;
            }
        };
        
        let mut delay = FreeRtos;
        
        info!("Configuring NRF24L01...");
        
        // Add initialization delay
        thread::sleep(Duration::from_millis(100));

        // Setup configuration
        let config = NrfConfig::default()
            .channel(76)
            .pa_level(PALevel::Max)
            .payload_size(32);

        info!("Creating NRF24L01 instance...");
        let mut nrf24 = match Nrf24l01::new(spi_device, ce_pin, &mut delay, config) {
            Ok(n) => {
                info!("NRF24L01 created successfully");
                n
            },
            Err(e) => {
                log::error!("NRF24L01 init failed: {:?}", e);
                log::error!("Check wiring: CS->GPIO10, CE->GPIO9, MOSI->GPIO7, MISO->GPIO2, SCLK->GPIO6");
                log::error!("Ensure NRF24L01 has stable 3.3V power supply");
                return;
            }
        };

        // Test if NRF24L01 is responding
        info!("Testing NRF24L01 connectivity...");
        match nrf24.is_connected() {
            Ok(true) => info!("✓ NRF24L01 is connected and responding"),
            Ok(false) => {
                log::error!("✗ NRF24L01 is not responding - check connections and power");
                return;
            },
            Err(e) => {
                log::error!("✗ Failed to check NRF24L01 connection: {:?}", e);
                return;
            }
        }

        // Set up for transmission
        if let Err(e) = nrf24.open_writing_pipe(b"Node1") {
            log::error!("Failed to open writing pipe: {:?}", e);
            return;
        }
        
        if let Err(e) = nrf24.stop_listening() {
            log::error!("Failed to stop listening: {:?}", e);
            return;
        }

        info!("NRF24L01 initialized successfully");

        // Define frequency ranges (in MHz relative to 2.4 GHz base)
        let frequency_ranges = vec![
            (2, 22),   // 2.402-2.422 GHz
            (7, 27),   // 2.407-2.427 GHz
            (12, 32),  // 2.412-2.432 GHz
            (17, 37),  // 2.417-2.437 GHz
            (22, 42),  // 2.422-2.442 GHz
        ];

        let mut range_index = 0;
        loop {
            let (start_channel, end_channel) = frequency_ranges[range_index];
            info!("Switching to frequency range: {:.3}-{:.3} GHz (channels {}-{})", 
                  2.4 + (start_channel as f32 / 1000.0), 
                  2.4 + (end_channel as f32 / 1000.0),
                  start_channel, 
                  end_channel);

            // Cycle through all channels in this range
            for channel in start_channel..=end_channel {
                // Set the channel
                if let Err(e) = nrf24.set_channel(channel as u8) {
                    log::error!("Failed to set channel {}: {:?}", channel, e);
                    continue;
                }

                // Generate and send noise data
                let noise_data = generate_random_noise(32);
                match nrf24.write(&mut delay, &noise_data) {
                    Ok(_) => info!("Sent noise on channel {} ({:.3} GHz): {:02X?}", 
                                  channel, 2.4 + (channel as f32 / 1000.0), 
                                  &noise_data[0..8]), // Show first 8 bytes
                    Err(e) => log::error!("Failed to send noise on channel {}: {:?}", channel, e),
                }

                // Small delay between channel hops (50ms)
                thread::sleep(Duration::from_millis(50));
            }

            // Move to next frequency range
            range_index = (range_index + 1) % frequency_ranges.len();
            
            // Pause before switching to next range
            thread::sleep(Duration::from_millis(500));
        }
    });

    Ok(())
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

    // Initialize NRF24L01 and start sending noise
    setup_nrf24l01_and_send_noise(
        peripherals.spi2,
        peripherals.pins.gpio6,
        peripherals.pins.gpio7,
        peripherals.pins.gpio2,
        peripherals.pins.gpio10,
        peripherals.pins.gpio9,
    ).unwrap();

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

    let wifi_configuration: WifiConfiguration = WifiConfiguration::Client(ClientConfiguration {
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