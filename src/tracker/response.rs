use std::{
    fmt::{Debug, Formatter},
    net::SocketAddr,
    time::Duration,
};

use anyhow::{Error, Result, anyhow};
use rand::RngCore;

use crate::bencoding::value::Value;

#[derive(Debug, PartialEq)]
pub struct TrackerResponse {
    pub complete: usize,
    pub incomplete: usize,
    pub interval: Duration,
    pub min_interval: Option<Duration>,
    pub peers: Vec<Peer>,
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
        let peers = {
            let peers: Vec<Value> = value.remove_entry("peers")?.try_into()?;
            let mut result = Vec::with_capacity(peers.len());
            for peer in peers {
                let peer = Peer::try_from(peer)?;
                result.push(peer);
            }
            result
        };
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
pub struct Peer {
    pub peer_id: Option<PeerId>,
    pub address: SocketAddr,
}

impl TryFrom<Value> for Peer {
    type Error = Error;

    fn try_from(mut value: Value) -> Result<Self> {
        let peer_id = match value.try_remove_entry("peer id")? {
            Some(Value::String(bytes)) => Some(bytes.try_into()?),
            _ => None,
        };
        let ip: String = value.remove_entry("ip")?.try_into()?;
        let port = value.remove_entry("port")?.try_into()?;
        let address = SocketAddr::new(ip.parse()?, port);
        Ok(Peer { peer_id, address })
    }
}

#[derive(PartialEq)]
pub struct PeerId(pub [u8; 20]);

impl PeerId {
    pub fn random() -> Self {
        let mut data = [0; 20];
        rand::rng().fill_bytes(&mut data);
        Self(data)
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
                peers: vec![Peer {
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
