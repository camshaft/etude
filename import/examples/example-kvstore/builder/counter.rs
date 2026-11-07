use upstream_codegen::service;

#[service(counter)]
extern "RPC" {
    /// Increments the counter and returns the new value
    fn increment(key: bytes::Bytes) -> u64;

    /// Decrements the counter and returns the new value
    fn decrement(key: bytes::Bytes) -> u64;

    /// Gets the current counter value
    fn get(key: bytes::Bytes) -> u64;

    /// Resets the counter to zero
    fn reset(key: bytes::Bytes);

    /// Adds to the current value, returning the previous value.
    fn fetch_add(key: bytes::Bytes, amount: u64) -> u64;
}
