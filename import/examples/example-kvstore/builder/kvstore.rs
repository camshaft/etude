use upstream_codegen::service;

#[service(kvstore)]
extern "RPC" {
    /// Get the value of key.
    ///
    /// Returns `None` if the key does not exist
    fn get(key: ::bytes::Bytes) -> Option<::bytes::Bytes>;

    /// Sets the given value for the key
    ///
    /// If key already exists, the old value is overwritten.
    fn set(key: ::bytes::Bytes, value: ::bytes::Bytes);

    /// Lists all of the keys stored in the store
    #[response(stream, header = usize)]
    fn keys() -> ::bytes::Bytes;

    #[response(stream, header = usize)]
    fn slow_keys(delay: core::time::Duration) -> ::bytes::Bytes;

    #[request(stream)]
    fn bulk_set(key: ::bytes::Bytes, value: ::bytes::Bytes);

    #[request(stream, header = usize)]
    fn bulk_set_with_count(key: ::bytes::Bytes, value: ::bytes::Bytes);

    #[request(stream)]
    #[response(stream)]
    fn bulk_get(key: ::bytes::Bytes) -> Option<::bytes::Bytes>;

    #[request(stream, header = usize)]
    #[response(stream, header = usize)]
    fn bulk_get_with_count(key: ::bytes::Bytes) -> Option<::bytes::Bytes>;

    /// Sets the given value for the key without waiting for a response
    ///
    /// This is a unidirectional method, so it doesn't return a response.
    #[unidirectional]
    fn set_no_wait(key: ::bytes::Bytes, value: ::bytes::Bytes);

    /// Deletes the given key
    fn del(key: ::bytes::Bytes);

    /// Pings the server
    fn ping(message: ::bytes::Bytes) -> ::bytes::Bytes;

    #[response(stream, header = usize)]
    fn fail(request: ::bytes::Bytes) -> ::bytes::Bytes;
}
