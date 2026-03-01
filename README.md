# http-logger

A simple HTTP logger that sends any log to an HTTP-POST endpoint.

Example Usage
---
```rust
fn main() {
    http_logger::init("http://localhost:8080".to_owned());
    log::info!("An Example Information!");
}
```

Example Request
---
This is an example request that this crate will make when calling log::info!(...); with a HttpLogger:
```http
POST / HTTP/1.1
Host: localhost:8080
Content-Length: 194
Content-Type: application/json; charset=UTF-8

{"timestamp":{"secs_since_epoch":1772354249,"nanos_since_epoch":368051819},"level":"INFO","target":"example","module":"example","file":"src/main.rs","line":4,"message":"An Example Information!"}
```
The structure in the body is public in this crate and can be used for deserialization on the server side.

Example Server
---
You can find an example (and very minimal) server implementation in the tests, but I highly advice one to use an async backend framework like actix-web or one of its many alternatives
