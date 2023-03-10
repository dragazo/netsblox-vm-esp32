use esp_idf_svc::http::client::{Configuration, EspHttpConnection};
use esp_idf_sys::EspError;
use embedded_svc::http::Method;

pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
    pub content_type: Option<String>,
}

/// Perform a single http(s) request and returns the response.
/// This function establishes a new HTTP(S) connection to the target server for every request.
/// While it is possible to create a client that issues multiple requests, there are currently
/// unresolved issues in esp-idf that result in corrupted response entries if the connection is cut by the server
/// (despite the issue being marked as closed). See https://github.com/espressif/esp-idf/issues/2684 for details.
pub fn http_request(method: Method, url: &str, headers: &[(&str, &str)], body: &[u8]) -> Result<Response, EspError> {
    let mut client = EspHttpConnection::new(&Configuration {
        use_global_ca_store: true,
        crt_bundle_attach: Some(esp_idf_sys::esp_crt_bundle_attach),
        ..Default::default()
    }).unwrap();

    let content_len_str = format!("{}", body.len());
    let mut aug_headers = Vec::with_capacity(headers.len() + 1);
    aug_headers.extend_from_slice(headers);
    aug_headers.push(("Content-Length", &content_len_str));

    client.initiate_request(method, url, &aug_headers)?;
    client.write(body)?;

    client.initiate_response()?;
    let status = client.status();
    let content_type = client.header("Content-Type").map(ToOwned::to_owned);

    let mut body = vec![];
    let mut buf = [0u8; 256];
    loop {
        let len = client.read(&mut buf)?;
        if len == 0 { break }
        body.extend_from_slice(&buf[..len]);
    }

    Ok(Response { status, body, content_type })
}
