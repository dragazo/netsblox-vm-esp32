use std::time::Duration;
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::wifi::{EspWifi, WifiWait};
use esp_idf_svc::nvs::{EspDefaultNvsPartition};
use esp_idf_svc::netif::{EspNetif, EspNetifWait};

use esp_idf_hal::modem::WifiModem;

use esp_idf_sys::EspError;

use embedded_svc::wifi::{AuthMethod, Configuration, ClientConfiguration, AccessPointConfiguration, Wifi as _};

use crate::storage::StorageController;

pub struct Wifi {
    wifi: EspWifi<'static>,
    event_loop: EspSystemEventLoop,
    storage: Arc<Mutex<StorageController>>,
}
impl Wifi {
    pub fn new(modem: WifiModem, event_loop: EspSystemEventLoop, nvs_partition: EspDefaultNvsPartition, storage: Arc<Mutex<StorageController>>) -> Result<Self, EspError> {
        Ok(Wifi {
            wifi: EspWifi::new(modem, event_loop.clone(), Some(nvs_partition))?,
            event_loop: event_loop,
            storage,
        })
    }
    pub fn connect(&mut self) -> Result<(), EspError> {
        let (ap_ssid, ap_pass, client_ssid, client_pass) = {
            let mut storage = self.storage.lock().unwrap();

            let ap_ssid = storage.wifi_ap_ssid().get()?;
            let ap_pass = storage.wifi_ap_pass().get()?;

            let client_ssid = storage.wifi_client_ssid().get()?;
            let client_pass = storage.wifi_client_pass().get()?;

            (ap_ssid, ap_pass, client_ssid, client_pass)
        };

        let ap_config = AccessPointConfiguration {
            ssid: ap_ssid.as_deref().unwrap_or("nb-esp32c3").into(),
            password: ap_pass.as_deref().unwrap_or("netsblox").into(),
            channel: 1,
            auth_method: AuthMethod::WPA2Personal,
            ..Default::default()
        };

        let client_config = match (client_ssid, client_pass) {
            (Some(ssid), Some(pass)) => {
                let aps = self.wifi.scan()?;
                let ap = aps.iter().find(|ap| ap.ssid.as_str() == ssid.as_str());

                Some(ClientConfiguration {
                    ssid: ssid.as_str().into(),
                    password: pass.as_str().into(),
                    channel: ap.map(|ap| ap.channel),
                    auth_method: ap.map(|ap| ap.auth_method).unwrap_or(AuthMethod::WPA2Personal),
                    ..Default::default()
                })
            }
            (_, _) => None,
        };
        let is_client = client_config.is_some();

        self.wifi.set_configuration(&match client_config {
            Some(client_config) => Configuration::Mixed(client_config, ap_config),
            None => Configuration::AccessPoint(ap_config),
        })?;

        self.wifi.start()?;
        let wait_for = || self.wifi.is_started().unwrap();
        if !WifiWait::new(&self.event_loop)?.wait_with_timeout(Duration::from_secs(10), wait_for) {
            panic!("wifi access point couldn't start");
        }

        if is_client {
            self.wifi.connect()?;
            let wait_for = || self.wifi.is_connected().unwrap() && self.wifi.sta_netif().get_ip_info().unwrap().ip != Ipv4Addr::new(0, 0, 0, 0);
            if !EspNetifWait::new::<EspNetif>(self.wifi.sta_netif(), &self.event_loop)?.wait_with_timeout(Duration::from_secs(10), wait_for) {
                println!("wifi client couldn't connect... wiping entry...");

                let mut storage = self.storage.lock().unwrap();
                storage.wifi_client_ssid().clear()?;
                storage.wifi_client_pass().clear()?;
            }
        }

        Ok(())
    }
    pub fn client_ip(&self) -> Option<Ipv4Addr> {
        let ip = self.wifi.sta_netif().get_ip_info().unwrap().ip;
        if ip != Ipv4Addr::new(0, 0, 0, 0) { Some(ip) } else { None }
    }
    pub fn server_ip(&self) -> Ipv4Addr {
        self.wifi.ap_netif().get_ip_info().unwrap().ip
    }
}
