use esp_idf_svc::http::client::{Configuration, EspHttpConnection};
use esp_idf_sys::EspError;
use embedded_svc::http::Method;

pub struct HttpClient {
    client: EspHttpConnection,
}
impl HttpClient {
    pub fn new() -> Self {
        let client = EspHttpConnection::new(&Configuration {
            use_global_ca_store: true,
            crt_bundle_attach: Some(esp_idf_sys::esp_crt_bundle_attach),
            ..Default::default()
        }).unwrap();

        Self { client }
    }
    pub fn request(&mut self, method: Method, url: &str, headers: &[(&str, &str)], body: &[u8]) -> Result<(u16, Vec<u8>), EspError> {
        self.client.initiate_request(method, url, headers)?;
        self.client.write(body)?;

        self.client.initiate_response()?;
        let status = self.client.status();

        let mut body = vec![];
        let mut buf = [0u8; 256];
        loop {
            let len = self.client.read(&mut buf)?;
            if len == 0 { break }
            body.extend_from_slice(&buf);
        }

        Ok((status, body))
    }
}
