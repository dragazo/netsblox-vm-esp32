use esp_idf_sys as _; // If using the `binstart` feature of `esp-idf-sys`, always keep this module imported

use std::time::Duration;
use std::net::Ipv4Addr;
use std::thread;

use esp_idf_svc::http::server::{EspHttpServer, EspHttpConnection};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::wifi::{EspWifi, WifiWait};
use esp_idf_svc::nvs::{EspDefaultNvs, EspDefaultNvsPartition};
use esp_idf_svc::netif::{EspNetif, EspNetifWait};

use esp_idf_hal::modem::WifiModem;

use esp_idf_sys::EspError;

use embedded_svc::http::Method;
use embedded_svc::http::server::{Handler, HandlerResult};
use embedded_svc::wifi::{AuthMethod, Configuration, ClientConfiguration, AccessPointConfiguration, Wifi as _};

// use netsblox_vm::project::Project;
// use netsblox_vm::ast::Parser;

const NVS_WIFI_SSID: &'static str = "wifi-ssid";
const NVS_WIFI_PASS: &'static str = "wifi-pass";

struct RootHandler;
impl Handler<EspHttpConnection<'_>> for RootHandler {
    fn handle(&self, connection: &mut EspHttpConnection<'_>) -> HandlerResult {
        connection.initiate_response(200, None, &[])?;
        connection.write(b"it's working!")?;
        Ok(())
    }
}

fn init_wifi(modem: WifiModem, event_loop: &EspSystemEventLoop, nvs_partition: EspDefaultNvsPartition, nvs: &EspDefaultNvs) -> Result<EspWifi<'static>, EspError> {
    let mut wifi = EspWifi::new(modem, event_loop.clone(), Some(nvs_partition))?;

    let is_client = nvs.contains(NVS_WIFI_SSID)? && nvs.contains(NVS_WIFI_PASS)?;
    let client_config = match is_client {
        true => {
            let mut ssid_buf = [0u8; 32];
            let mut pass_buf = [0u8; 64];

            let ssid = std::str::from_utf8(nvs.get_raw(NVS_WIFI_SSID, &mut ssid_buf)?.unwrap()).unwrap();
            let pass = std::str::from_utf8(nvs.get_raw(NVS_WIFI_PASS, &mut pass_buf)?.unwrap()).unwrap();

            let aps = wifi.scan()?;
            let ap = aps.iter().find(|ap| ap.ssid == ssid);

            Some(ClientConfiguration {
                ssid: ssid.into(),
                password: pass.into(),
                channel: ap.map(|ap| ap.channel),
                auth_method: ap.map(|ap| ap.auth_method).unwrap_or(AuthMethod::WPA2Personal),
                ..Default::default()
            })
        }
        false => None,
    };
    let ap_config = AccessPointConfiguration {
        ssid: "nb-esp32c3".into(),
        password: "netsblox".into(),
        channel: 1,
        auth_method: AuthMethod::WPA2Personal,
        ..Default::default()
    };

    wifi.set_configuration(&match client_config {
        Some(client_config) => Configuration::Mixed(client_config, ap_config),
        None => Configuration::AccessPoint(ap_config),
    })?;

    wifi.start()?;
    let wait_for = || wifi.is_started().unwrap();
    if !WifiWait::new(event_loop)?.wait_with_timeout(Duration::from_secs(20), wait_for) {
        panic!("wifi didn't start");
    }

    if is_client {
        wifi.connect()?;
        let wait_for = || wifi.is_connected().unwrap() && wifi.sta_netif().get_ip_info().unwrap().ip != Ipv4Addr::new(0, 0, 0, 0);
        if !EspNetifWait::new::<EspNetif>(wifi.sta_netif(), &event_loop)?.wait_with_timeout(Duration::from_secs(20), wait_for) {
            panic!("wifi couldn't connect");
        }
    }

    Ok(wifi)
}

fn main() {
    // It is necessary to call this function once. Otherwise some patches to the runtime
    // implemented by esp-idf-sys might not link properly. See https://github.com/esp-rs/esp-idf-template/issues/71
    esp_idf_sys::link_patches();

    let modem = unsafe { WifiModem::new() }; // safe because we only do this once (singleton)
    let event_loop = EspSystemEventLoop::take().unwrap();
    let nvs_partition = EspDefaultNvsPartition::take().unwrap();
    let nvs = EspDefaultNvs::new(nvs_partition.clone(), "nb", true).unwrap();

    let wifi = init_wifi(modem, &event_loop, nvs_partition, &nvs).unwrap();
    println!("wifi sta ip: {}", wifi.sta_netif().get_ip_info().unwrap().ip);
    println!("wifi ap  ip: {}", wifi.ap_netif().get_ip_info().unwrap().ip);

    let mut server = EspHttpServer::new(&Default::default()).unwrap();
    server.handler("/", Method::Get, RootHandler).unwrap();

    // let ast = Parser::builder().build().unwrap().parse(include_str!("../test.xml")).unwrap();
    // println!("ast: {ast:?}");

    loop {
        thread::sleep(Duration::from_secs(1));
    }
}
