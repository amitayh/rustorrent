use rand::RngCore;

#[derive(PartialEq, Clone)]
pub struct PeerId(pub [u8; 20]);

impl PeerId {
    pub fn random() -> Self {
        let mut data = [0; 20];
        rand::rng().fill_bytes(&mut data);
        Self(data)
    }
}
