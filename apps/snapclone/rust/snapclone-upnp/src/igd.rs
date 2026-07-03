//! IGD XML description parsing and SOAP AddPortMapping.

use anyhow::{bail, Context, Result};
use tracing::{debug, info};

/// Parsed IGD service info needed to make port mapping requests.
#[derive(Debug, Clone)]
pub struct IgdService {
    pub control_url:  String,
    pub service_type: String,
}

/// Parse the IGD description XML to find the WANIPConnection control URL.
/// Ported from UPnPPortMapper::parseDescriptionXml().
pub fn parse_description(xml: &str, base_url: &str) -> Option<IgdService> {
    // Look for WANIPConnection service type and extract controlURL
    let xml_lower = xml.to_ascii_lowercase();

    // Find WANIPConnection service block
    let svc_markers = ["wanipconnection", "wanpppconnection"];
    for marker in &svc_markers {
        if let Some(pos) = xml_lower.find(marker) {
            // Find controlURL within the service block
            let block = &xml[pos..];
            if let Some(ctrl_start) = block.to_ascii_lowercase().find("<controlurl>") {
                let after = &block[ctrl_start + 12..];
                if let Some(ctrl_end) = after.to_ascii_lowercase().find("</controlurl>") {
                    let relative = after[..ctrl_end].trim();
                    // Resolve relative URL against base
                    let control_url = if relative.starts_with("http") {
                        relative.to_owned()
                    } else {
                        // Extract host from base_url
                        let base = base_url.trim_end_matches('/');
                        let host_end = base[8..].find('/').map(|i| i + 8).unwrap_or(base.len());
                        format!("{}{}", &base[..host_end], relative)
                    };
                    return Some(IgdService {
                        control_url,
                        service_type: marker.to_string(),
                    });
                }
            }
        }
    }
    None
}

/// Send a UPnP SOAP AddPortMapping request.
pub async fn add_mapping(
    service: &IgdService,
    internal_ip: &str,
    port: u16,
    lease_secs: u32,
) -> Result<()> {
    let soap_body = format!(
        r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
  <s:Body>
    <u:AddPortMapping xmlns:u="urn:schemas-upnp-org:service:WANIPConnection:1">
      <NewRemoteHost></NewRemoteHost>
      <NewExternalPort>{port}</NewExternalPort>
      <NewProtocol>TCP</NewProtocol>
      <NewInternalPort>{port}</NewInternalPort>
      <NewInternalClient>{ip}</NewInternalClient>
      <NewEnabled>1</NewEnabled>
      <NewPortMappingDescription>SnapClone</NewPortMappingDescription>
      <NewLeaseDuration>{lease}</NewLeaseDuration>
    </u:AddPortMapping>
  </s:Body>
</s:Envelope>"#,
        port = port, ip = internal_ip, lease = lease_secs
    );

    let client = reqwest_lite::post(&service.control_url, &soap_body)?;
    if !client.status_ok() {
        bail!("UPnP AddPortMapping failed: HTTP {}", client.status);
    }
    info!("UPnP: mapped port {} → {} (lease {}s)", port, internal_ip, lease_secs);
    Ok(())
}

// Minimal HTTP POST without reqwest dependency (keep crate small)
mod reqwest_lite {
    use anyhow::{bail, Result};
    use std::io::{Read, Write};
    use std::net::TcpStream;

    pub struct Response { pub status: u32, pub body: String }
    impl Response { pub fn status_ok(&self) -> bool { self.status == 200 } }

    pub fn post(url: &str, body: &str) -> Result<Response> {
        // Parse URL
        let no_scheme = url.strip_prefix("http://").unwrap_or(url);
        let (host_port, path) = no_scheme.split_once('/').unwrap_or((no_scheme, ""));
        let path = format!("/{}", path);

        let mut stream = TcpStream::connect(host_port)?;
        stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;

        let request = format!(
            "POST {} HTTP/1.0\r\nHost: {}\r\n\
             Content-Type: text/xml; charset=\"utf-8\"\r\n\
             SOAPAction: \"urn:schemas-upnp-org:service:WANIPConnection:1#AddPortMapping\"\r\n\
             Content-Length: {}\r\n\r\n{}",
            path, host_port, body.len(), body
        );
        stream.write_all(request.as_bytes())?;

        let mut response = String::new();
        stream.read_to_string(&mut response)?;

        let status: u32 = response.lines().next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        Ok(Response { status, body: response })
    }
}
