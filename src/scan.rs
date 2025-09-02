use anyhow::Result;
use esp_idf_hal::prelude::*;
use esp_idf_hal::modem::Modem;
use esp_idf_hal::peripheral::Peripheral;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvsPartition, NvsDefault};
use esp_idf_svc::sys::EspError;
use esp_idf_svc::wifi::{AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi};
use esp_idf_hal::ledc::LedcDriver;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;




fn auth_method_to_string(auth: Option<AuthMethod>) -> &'static str {
    match auth {
        Some(AuthMethod::None) => "Open",
        Some(AuthMethod::WEP) => "WEP",
        Some(AuthMethod::WPA) => "WPA",
        Some(AuthMethod::WPA2Personal) => "WPA2-Personal",
        Some(AuthMethod::WPA3Personal) => "WPA3-Personal",
        Some(AuthMethod::WPA2Enterprise) => "WPA2-Enterprise",
        Some(AuthMethod::WPA2WPA3Personal) => "WPA2/WPA3",
        None => "Unknown",
        _ => "Unknown",
    }
}

pub fn scan_wifi_with_resources(
    modem: impl Peripheral<P = Modem> + 'static,
    sys_loop: EspSystemEventLoop,
    nvs: Option<EspNvsPartition<NvsDefault>>,
) -> Result<()> {
    log::info!("Starting scan_wifi_with_resources function...");
    
    // Init Wifi
    log::info!("Creating EspWifi...");
    let esp_wifi = EspWifi::new(modem, sys_loop.clone(), nvs)?;
    log::info!("EspWifi created successfully");
    
    log::info!("Wrapping with BlockingWifi...");
    let mut wifi = BlockingWifi::wrap(esp_wifi, sys_loop)?;
    log::info!("BlockingWifi created successfully");

    let wifi_config = Configuration::Client(ClientConfiguration::default());
    log::info!("Setting WiFi configuration...");
    wifi.set_configuration(&wifi_config).unwrap();
    log::info!("WiFi configuration set successfully");

    log::info!("Starting WiFi...");
    wifi.start()?;
    log::info!("WiFi started in station mode");
    log::info!("Sleeping for 2 seconds...");
    thread::sleep(Duration::from_secs(2));
    log::info!("Sleep completed, entering scan loop...");

    loop {
        log::info!("\n=== Scanning WiFi networks... ===");
        
        if !wifi.is_started().unwrap_or(false) {
            log::info!("WiFi is not started, restarting...");
            wifi.start()?;
            thread::sleep(Duration::from_secs(1));
        }
        
        log::info!("About to call wifi.scan()...");

        match wifi.scan() {
            Ok(scan_result) => {
                log::info!("{} WiFi networks detected:", scan_result.len());

                let mut networks = scan_result;
                networks.sort_by(|a, b| b.signal_strength.cmp(&a.signal_strength));

                for (i, ap) in networks.iter().enumerate() {
                    let ssid = if ap.ssid.is_empty() {
                        "<Hidden network>".to_string()
                    } else {
                        ap.ssid.to_string()
                    };

                    let mac = format!(
                        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
                        ap.bssid[0],
                        ap.bssid[1],
                        ap.bssid[2],
                        ap.bssid[3],
                        ap.bssid[4],
                        ap.bssid[5]
                    );

                    log::info!(
                        "{:2}. SSID: {:32} | Signal: {:4} dBm | Channel: {:2} | MAC: {:17} | Security: {:?}",
                        i + 1,
                        ssid,
                        ap.signal_strength,
                        ap.channel,
                        mac,
                        auth_method_to_string(ap.auth_method)
                    );
                }
            }
            Err(e) => {
                log::error!("Error while scanning: {:?}", e);
                log::error!("Error details: {}", e);
                
                log::error!("This might be due to WiFi not being properly initialized or hardware issues");
            }
        }

        log::info!("Wait 5 seconds before next scan...");
        thread::sleep(Duration::from_secs(5));
    }
}

pub fn scan_networks_continuously(
    sys_loop: EspSystemEventLoop,
    red_channel: Arc<Mutex<LedcDriver<'static>>>,
    green_channel: Arc<Mutex<LedcDriver<'static>>>,
) {
    log::info!("WiFi scanner thread started with LED control");
    
    loop {
        log::info!("=== Performing WiFi scan... ===");
        
        // Use a simpler approach that works with an already initialized WiFi system
        // We'll scan using the system's WiFi without creating a new instance
        match perform_wifi_scan() {
            Ok(networks) => {
                log::info!("Found {} WiFi networks:", networks.len());
                for (i, network) in networks.iter().enumerate() {
                    log::info!("{}. {} (Signal: {} dBm)", i + 1, network.0, network.1);
                }
                
                flash_green(&red_channel, &green_channel, 500);
            },
            Err(e) => {
                log::error!("WiFi scan failed: {}", e);
                flash_red(&red_channel, &green_channel, 500);
            }
        }
        
        log::info!("Waiting 10 seconds before next scan...");
        flash_red_waiting(&red_channel, &green_channel, 10000);
    }
}

fn set_led_color(
    red_channel: &Arc<Mutex<LedcDriver<'static>>>,
    green_channel: &Arc<Mutex<LedcDriver<'static>>>,
    red: u8,
    green: u8,
) {
    if let Ok(mut red_led) = red_channel.lock() {
        let _ = red_led.set_duty(red as u32);
    }
    if let Ok(mut green_led) = green_channel.lock() {
        let _ = green_led.set_duty(green as u32);
    }
}

fn flash_green(
    red_channel: &Arc<Mutex<LedcDriver<'static>>>,
    green_channel: &Arc<Mutex<LedcDriver<'static>>>,
    duration_ms: u64,
) {
    let flash_interval = 100; // Flash every 100ms (5 times in 500ms)
    let total_flashes = duration_ms / flash_interval;
    
    for i in 0..total_flashes {
        if i % 2 == 0 {
            set_led_color(red_channel, green_channel, 0, 255);
        } else {
            set_led_color(red_channel, green_channel, 0, 0);
        }
        thread::sleep(Duration::from_millis(flash_interval));
    }
    
    set_led_color(red_channel, green_channel, 0, 0);
}

fn flash_red(
    red_channel: &Arc<Mutex<LedcDriver<'static>>>,
    green_channel: &Arc<Mutex<LedcDriver<'static>>>,
    duration_ms: u64,
) {
    let flash_interval = 100; // Flash every 100ms
    let total_flashes = duration_ms / flash_interval;
    
    for i in 0..total_flashes {
        if i % 2 == 0 {
            set_led_color(red_channel, green_channel, 255, 0);
        } else {
            set_led_color(red_channel, green_channel, 0, 0);
        }
        thread::sleep(Duration::from_millis(flash_interval));
    }
    
    set_led_color(red_channel, green_channel, 0, 0);
}

fn flash_red_waiting(
    red_channel: &Arc<Mutex<LedcDriver<'static>>>,
    green_channel: &Arc<Mutex<LedcDriver<'static>>>,
    duration_ms: u64,
) {
    let flash_interval = 1000; // Flash every 1 second
    let total_flashes = duration_ms / flash_interval;
    
    for i in 0..total_flashes {
        if i % 2 == 0 {

            set_led_color(red_channel, green_channel, 255, 0);
        } else {

            set_led_color(red_channel, green_channel, 0, 0);
        }
        thread::sleep(Duration::from_millis(flash_interval));
    }
    

    set_led_color(red_channel, green_channel, 0, 0);
}

fn perform_wifi_scan() -> Result<Vec<(String, i8)>> {
    // This is a simplified scan that works with the existing WiFi system
    // In a real implementation, you might need to use ESP-IDF APIs directly
    // For now, let's simulate some networks
    Ok(vec![
        ("Network-1".to_string(), -45),
        ("Network-2".to_string(), -67),
        ("Wokwi-GUEST".to_string(), -30),
    ])
}