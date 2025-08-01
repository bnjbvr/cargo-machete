// Import using the actual crate name (webpki) not the package name (rustls-webpki)
// Use webpki in a simple way that will be detected

pub fn validate_cert() {
    // This will be detected as using webpki
    webpki::verify_cert();
    println!("Using webpki for certificate validation");
}
