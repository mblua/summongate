/// Dedicated token for WebSocket authentication.
/// Separate from MasterToken to limit blast radius if captured via network sniffing.
pub struct WebAccessToken(String);

impl WebAccessToken {
    pub fn new(token: String) -> Self {
        Self(token)
    }

    /// Constant-time comparison to prevent timing oracle attacks.
    pub fn matches(&self, candidate: &str) -> bool {
        let a = self.0.as_bytes();
        let b = candidate.as_bytes();
        if a.len() != b.len() {
            return false;
        }
        a.iter()
            .zip(b.iter())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
    }

    pub fn value(&self) -> &str {
        &self.0
    }
}
