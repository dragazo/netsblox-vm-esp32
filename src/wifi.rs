use std::time::Duration;

use embedded_svc::wifi::{self as wifi_svc, AuthMethod, Configuration, ClientConfiguration, Wifi as _};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::EspWifi;
use esp_idf_sys::EspError;
use esp_idf_hal::modem::WifiModem;

#[derive(Debug)]
pub enum WifiError {
    MissingName,
    MissingPassword,
    EspError(EspError),
}
impl From<EspError> for WifiError { fn from(e: EspError) -> Self { Self::EspError(e) } }

pub struct Wifi<'d> {
    wifi: EspWifi<'d>,
}
impl<'d> Wifi<'d> {
    pub fn new(modem: WifiModem, event_loop: EspSystemEventLoop, nvs: Option<EspDefaultNvsPartition>, ssid: &str, password: &str, auth_method: AuthMethod) -> Result<Self, WifiError> {
        if ssid.is_empty() {
            return Err(WifiError::MissingName);
        }
        if password.is_empty() && auth_method != AuthMethod::None {
            return Err(WifiError::MissingPassword);
        }

        let mut wifi = EspWifi::new(modem, event_loop, nvs)?;

        let ap_infos = wifi.scan()?;
        let channel = ap_infos.into_iter().find(|a| a.ssid == ssid).map(|x| x.channel);

        wifi.set_configuration(&Configuration::Client(ClientConfiguration {
            ssid: ssid.into(),
            password: password.into(),
            channel,
            auth_method,
            ..Default::default()
        }))?;

        // wifi.start

        // wifi.wait_status_with_timeout(Duration::from_secs(2100), |status| {
        //     !status.is_transitional()
        // })
        // .map_err(|err| anyhow::anyhow!("Unexpected Wifi status (Transitional state): {:?}", err))?;

        // let status = wifi.get_status();

        // if let wifi_svc::Status(
        //     ClientStatus::Started(ClientConnectionStatus::Connected(ClientIpStatus::Done(
        //         _ip_settings,
        //     ))),
        //     _,
        // ) = status
        // {
        //     info!("Wifi connected");
        // } else {
        //     bail!(
        //         "Could not connect to Wifi - Unexpected Wifi status: {:?}",
        //         status
        //     );
        // }

        Ok(Wifi { wifi })
    }
}
