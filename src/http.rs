use esp_idf_svc::http::client::{Configuration, EspHttpConnection};
use esp_idf_sys::EspError;
use embedded_svc::http::Method;

pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
    pub content_type: Option<String>,
}

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
    pub fn request(&mut self, method: Method, url: &str, headers: &[(&str, &str)], body: &[u8]) -> Result<Response, EspError> {
        fn log_heap() {
            let (free_heap_size, internal_heap_size) = unsafe {
                (esp_idf_sys::esp_get_free_heap_size(), esp_idf_sys::esp_get_free_internal_heap_size())
            };
            println!("heap info {free_heap_size} : {internal_heap_size}");
        }
        println!("starting request: {url}");
        log_heap();

        let content_len_str = format!("{}", body.len());
        let mut aug_headers = Vec::with_capacity(headers.len() + 1);
        aug_headers.extend_from_slice(headers);
        aug_headers.push(("Content-Length", &content_len_str));

        println!("here");
        if self.client.is_request_initiated() {
            println!("there");
            let _ = self.client.initiate_response(); // if in request state, transition to response state and ignore errors (caused by previous error)
            println!("aft inner");
        }
        println!("before request");
        self.client.initiate_request(method, url, &aug_headers)?;
        println!("aft request");
        self.client.write(body)?;
        println!("aft body");

        self.client.initiate_response()?;
        let status = self.client.status();
        let content_type = self.client.header("Content-Type").map(ToOwned::to_owned);

        let mut body = vec![];
        let mut buf = [0u8; 256];
        loop {
            let len = self.client.read(&mut buf)?;
            if len == 0 { break }
            body.extend_from_slice(&buf[..len]);
        }

        Ok(Response { status, body, content_type })
    }
}
