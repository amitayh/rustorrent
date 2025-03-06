use std::{
    fmt::{Debug, Formatter},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};

use anyhow::{Error, Result, anyhow};

use crate::{bencoding::value::Value, peer::PeerId};

const ADDR_LEN: usize = 6;

#[derive(Debug, PartialEq)]
pub struct TrackerResponse {
    pub complete: usize,
    pub incomplete: usize,
    pub interval: Duration,
    pub min_interval: Option<Duration>,
    pub peers: Vec<PeerInfo>,
}

impl TryFrom<Value> for TrackerResponse {
    type Error = Error;

    fn try_from(mut value: Value) -> Result<Self> {
        let complete = value.remove_entry("complete")?.try_into()?;
        let incomplete = value.remove_entry("incomplete")?.try_into()?;
        let interval = value.remove_entry("interval")?.try_into()?;
        let min_interval = match value.try_remove_entry("min interval")? {
            Some(value) => Some(value.try_into()?),
            None => None,
        };
        let peers = match value.remove_entry("peers")? {
            Value::String(peers) if peers.len() % ADDR_LEN == 0 => {
                let mut result = Vec::with_capacity(peers.len() / ADDR_LEN);
                for i in (0..peers.len()).step_by(ADDR_LEN) {
                    let mut bytes = [0; ADDR_LEN];
                    bytes.copy_from_slice(&peers[i..(i + ADDR_LEN)]);
                    result.push(PeerInfo::from(bytes));
                }
                Ok(result)
            }
            Value::List(peers) => {
                let mut result = Vec::with_capacity(peers.len());
                for peer in peers {
                    let peer = PeerInfo::try_from(peer)?;
                    result.push(peer);
                }
                Ok(result)
            }
            value => Err(anyhow!("unexpected value for peers: {:?}", value)),
        }?;
        Ok(TrackerResponse {
            complete,
            incomplete,
            interval,
            min_interval,
            peers,
        })
    }
}

#[derive(Debug, PartialEq)]
pub struct PeerInfo {
    pub peer_id: Option<PeerId>,
    pub address: SocketAddr,
}

impl TryFrom<Value> for PeerInfo {
    type Error = Error;

    fn try_from(mut value: Value) -> Result<Self> {
        let peer_id = match value.try_remove_entry("peer id")? {
            Some(Value::String(bytes)) => Some(bytes.try_into()?),
            _ => None,
        };
        let ip: String = value.remove_entry("ip")?.try_into()?;
        let port = value.remove_entry("port")?.try_into()?;
        let address = SocketAddr::new(ip.parse()?, port);
        Ok(PeerInfo { peer_id, address })
    }
}

impl From<[u8; 6]> for PeerInfo {
    fn from(value: [u8; 6]) -> Self {
        let ip = Ipv4Addr::new(value[0], value[1], value[2], value[3]);
        let port = {
            let mut bytes = [0; 2];
            bytes.copy_from_slice(&value[4..]);
            u16::from_be_bytes(bytes)
        };
        let address = SocketAddr::new(IpAddr::V4(ip), port);
        PeerInfo {
            peer_id: None,
            address,
        }
    }
}

impl TryFrom<Vec<u8>> for PeerId {
    type Error = Error;

    fn try_from(value: Vec<u8>) -> Result<Self> {
        let bytes = value
            .try_into()
            .map_err(|err| anyhow!("invalid peer id {:?}", err))?;
        Ok(PeerId(bytes))
    }
}

impl Debug for PeerId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match String::from_utf8(self.0.to_vec()) {
            Ok(string) => write!(f, "{:?}", string),
            Err(_) => write!(f, "<peer id={}>", hex::encode(self.0)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_tracker_response() {
        let peer_id = "-TR3000-47qm0ov7eav4";
        let body = Value::dictionary()
            .with_entry("complete", Value::Integer(12))
            .with_entry("incomplete", Value::Integer(34))
            .with_entry("interval", Value::Integer(1800))
            .with_entry(
                "peers",
                Value::list().with_value(
                    Value::dictionary()
                        .with_entry("ip", Value::string("12.34.56.78"))
                        .with_entry("peer id", Value::string(&peer_id))
                        .with_entry("port", Value::Integer(51413)),
                ),
            );

        let response = TrackerResponse::try_from(body).expect("invalid response body");

        assert_eq!(
            response,
            TrackerResponse {
                complete: 12,
                incomplete: 34,
                interval: Duration::from_secs(1800),
                min_interval: None,
                peers: vec![PeerInfo {
                    peer_id: Some(PeerId(peer_id.as_bytes().try_into().unwrap())),
                    address: "12.34.56.78:51413".parse().unwrap()
                }]
            }
        );
    }

    #[test]
    fn support_peer_ip_v6() {
        let body = Value::dictionary()
            .with_entry("complete", Value::Integer(12))
            .with_entry("incomplete", Value::Integer(34))
            .with_entry("interval", Value::Integer(1800))
            .with_entry(
                "peers",
                Value::list().with_value(
                    Value::dictionary()
                        .with_entry("ip", Value::string("2600:1702:6aa3:b210::72"))
                        .with_entry("port", Value::Integer(51413)),
                ),
            );

        let response = TrackerResponse::try_from(body).expect("invalid response body");

        assert_eq!(
            response.peers[0].address,
            "[2600:1702:6aa3:b210::72]:51413".parse().unwrap()
        );
    }

    #[test]
    fn support_compact_response() {
        let body = Value::dictionary()
            .with_entry("complete", Value::Integer(12))
            .with_entry("incomplete", Value::Integer(34))
            .with_entry("interval", Value::Integer(1800))
            .with_entry(
                "peers",
                Value::String(b"\x01\x02\x03\x04\x04\xD2\x05\x06\x07\x08\x16\x2E".to_vec()),
            );

        let response = TrackerResponse::try_from(body).expect("invalid response body");

        assert_eq!(
            response.peers,
            vec![
                PeerInfo {
                    peer_id: None,
                    address: "1.2.3.4:1234".parse().unwrap(),
                },
                PeerInfo {
                    peer_id: None,
                    address: "5.6.7.8:5678".parse().unwrap(),
                }
            ]
        );
    }

    #[test]
    fn support_min_interval() {
        let body = Value::dictionary()
            .with_entry("complete", Value::Integer(12))
            .with_entry("incomplete", Value::Integer(34))
            .with_entry("interval", Value::Integer(1800))
            .with_entry("min interval", Value::Integer(900))
            .with_entry("peers", Value::list());

        let response = TrackerResponse::try_from(body).expect("invalid response body");

        assert_eq!(response.min_interval, Some(Duration::from_secs(900)));
    }

    // TODO: support peer binary model
}
