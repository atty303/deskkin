use std::io::{Read, Write};

use serde::Deserialize;
use zeroize::{Zeroize, Zeroizing};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    schema_version: u8,
    ssid: String,
    password: String,
    host_ipv4: String,
}

impl Drop for Profile {
    fn drop(&mut self) {
        self.ssid.zeroize();
        self.password.zeroize();
        self.host_ipv4.zeroize();
    }
}

fn main() -> Result<(), &'static str> {
    let mut input = Zeroizing::new(Vec::new());
    std::io::stdin()
        .read_to_end(&mut input)
        .map_err(|_| "profile_schema_invalid")?;
    let profile: Profile = serde_json::from_slice(&input).map_err(|_| "profile_schema_invalid")?;
    if profile.schema_version != 1
        || !(1..=32).contains(&profile.ssid.len())
        || !(8..=63).contains(&profile.password.len())
        || !profile
            .password
            .bytes()
            .all(|byte| (0x20..=0x7e).contains(&byte))
    {
        return Err("profile_schema_invalid");
    }
    let address: std::net::Ipv4Addr = profile
        .host_ipv4
        .parse()
        .map_err(|_| "profile_schema_invalid")?;
    let octets = address.octets();
    if !deskkin_desktop_host::is_exact_private_lan_address(std::net::SocketAddr::from((
        address,
        deskkin_desktop_host::PRIVATE_LAN_PORT,
    ))) {
        return Err("profile_schema_invalid");
    }
    let mut payload = Zeroizing::new(Vec::with_capacity(
        2 + profile.ssid.len() + profile.password.len() + 6,
    ));
    payload.push(u8::try_from(profile.ssid.len()).map_err(|_| "profile_schema_invalid")?);
    payload.extend_from_slice(profile.ssid.as_bytes());
    payload.push(u8::try_from(profile.password.len()).map_err(|_| "profile_schema_invalid")?);
    payload.extend_from_slice(profile.password.as_bytes());
    payload.extend_from_slice(&octets);
    payload.extend_from_slice(&deskkin_desktop_host::PRIVATE_LAN_PORT.to_be_bytes());
    std::io::stdout()
        .write_all(&payload)
        .map_err(|_| "profile_schema_invalid")
}
