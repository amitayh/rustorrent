use sha1::Digest;

use crate::{bencoding::value::Value, codec::Encoder};

#[derive(PartialEq, Clone)]
pub struct Md5(pub [u8; 16]);

impl std::fmt::Debug for Md5 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Md5(")?;
        for byte in self.0 {
            write!(f, "{:02x}", byte)?;
        }
        write!(f, ")")
    }
}

#[derive(PartialEq, Clone)]
pub struct Sha1(pub [u8; 20]);

impl Sha1 {
    pub fn from_hex(hex: &str) -> Option<Self> {
        let bytes = hex::decode(hex).ok()?;
        let bytes = bytes.try_into().ok()?;
        Some(Self(bytes))
    }
}

impl std::fmt::Debug for Sha1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Sha1(")?;
        for byte in self.0 {
            write!(f, "{:02x}", byte)?;
        }
        write!(f, ")")
    }
}

impl From<&Value> for Sha1 {
    fn from(value: &Value) -> Self {
        let mut hasher = sha1::Sha1::new();
        value.encode(&mut hasher).expect("hashing shouldn't fail");
        let sha1 = hasher.finalize();
        Sha1(sha1.into())
    }
}
