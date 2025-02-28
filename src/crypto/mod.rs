#[derive(PartialEq)]
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

#[derive(PartialEq)]
pub struct Sha1(pub [u8; 20]);

impl std::fmt::Debug for Sha1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Sha1(")?;
        for byte in self.0 {
            write!(f, "{:02x}", byte)?;
        }
        write!(f, ")")
    }
}
