use bytes::Bytes;
use upstream_core::{
    runtime::{Env, Result},
    service::{
        RequestStream, ResponseStream,
        stream::{self, StreamExt},
    },
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

#[derive(Clone, Default)]
pub struct KVStore(Arc<Mutex<HashMap<Bytes, Bytes>>>);

impl<PeerAddr: 'static + Send + Sync> crate::service::kvstore::Provider<PeerAddr> for KVStore {
    async fn bulk_get<E: Env>(
        &self,
        _env: &E,
        items: impl RequestStream<Bytes>,
    ) -> Result<impl ResponseStream<Option<Bytes>>> {
        use stream::StreamExt;

        // Map each key in the stream to its value in the store
        let store = self.0.clone();
        let response_stream =
            items.map(move |result| result.map(|key| store.lock().unwrap().get(&key).cloned()));

        Ok(response_stream)
    }

    async fn bulk_get_with_count<E: Env>(
        &self,
        _env: &E,
        count: usize,
        items: impl RequestStream<Bytes>,
    ) -> Result<(usize, impl ResponseStream<Option<Bytes>>)> {
        use stream::StreamExt;

        // this could be a hint to how much work we're requesting
        let _ = count;

        // Map each key in the stream to its value in the store
        let store = self.0.clone();

        let total_len = store.lock().unwrap().len();

        let response_stream =
            items.map(move |result| result.map(|key| store.lock().unwrap().get(&key).cloned()));

        Ok((total_len, response_stream))
    }

    async fn bulk_set_with_count<E: Env>(
        &self,
        _env: &E,
        count: usize,
        items: impl RequestStream<(Bytes, Bytes)>,
    ) -> Result<()> {
        use stream::StreamExt;

        // the client is telling us how many items it's about to send so reserve some space for it
        self.0.lock().unwrap().reserve(count);

        // Process the stream of key-value pairs
        let store = self.0.clone();
        let mut stream = Box::pin(items);

        while let Some(result) = stream.next().await {
            match result {
                Ok((key, value)) => {
                    let _ = store.lock().unwrap().insert(key, value);
                }
                Err(e) => return Err(e),
            }
        }

        Ok(())
    }

    async fn bulk_set<E: Env>(
        &self,
        _env: &E,
        items: impl RequestStream<(Bytes, Bytes)>,
    ) -> Result<()> {
        use stream::StreamExt;

        // Process the stream of key-value pairs
        let store = self.0.clone();
        let mut stream = Box::pin(items);

        while let Some(result) = stream.next().await {
            match result {
                Ok((key, value)) => {
                    let _ = store.lock().unwrap().insert(key, value);
                }
                Err(e) => return Err(e),
            }
        }

        Ok(())
    }

    async fn del<E: Env>(&self, _env: &E, key: bytes::Bytes) -> Result<()> {
        let _ = self.0.lock().unwrap().remove(&key);
        Ok(())
    }

    async fn get<E: Env>(&self, _env: &E, key: bytes::Bytes) -> Result<Option<bytes::Bytes>> {
        Ok(self.0.lock().unwrap().get(&key).cloned())
    }

    async fn ping<E: Env>(&self, _env: &E, message: bytes::Bytes) -> Result<bytes::Bytes> {
        Ok(message)
    }

    async fn set<E: Env>(&self, _env: &E, key: bytes::Bytes, value: bytes::Bytes) -> Result<()> {
        let _ = self.0.lock().unwrap().insert(key, value);
        Ok(())
    }

    async fn set_no_wait<E: Env>(
        &self,
        _env: &E,
        key: bytes::Bytes,
        value: bytes::Bytes,
    ) -> Result<()> {
        let _ = self.0.lock().unwrap().insert(key, value);
        Ok(())
    }

    async fn keys<E: Env>(&self, _env: &E) -> Result<(usize, impl ResponseStream<Bytes>)> {
        // Clone the keys to avoid holding the lock during stream processing
        let keys: Vec<Bytes> = self.0.lock().unwrap().keys().cloned().collect();

        // let the client know how many items we'll be streaming
        let len = keys.len();

        // Return a stream from the keys
        let stream = stream::iter(keys.into_iter().map(Ok));
        Ok((len, stream))
    }

    async fn slow_keys<E: Env>(
        &self,
        env: &E,
        delay: core::time::Duration,
    ) -> Result<(usize, impl ResponseStream<Bytes>)> {
        // Clone the keys to avoid holding the lock during stream processing
        let keys: Vec<Bytes> = self.0.lock().unwrap().keys().cloned().collect();

        // let the client know how many items we'll be streaming
        let len = keys.len();

        // Return a stream from the keys
        let stream = stream::iter(keys.into_iter().map(Ok));

        let env = env.clone();
        let stream = stream.then(move |res| {
            let env = env.clone();
            async move {
                env.sleep(delay).await;
                res
            }
        });

        Ok((len, stream))
    }

    async fn fail<E: Env>(
        &self,
        _env: &E,
        request: bytes::Bytes,
    ) -> Result<(usize, impl ResponseStream<bytes::Bytes>)> {
        <Result<(usize, stream::Empty<_>)>>::Err(upstream_core::runtime::Error::message(
            format_args!("{request:?}"),
        ))
    }
}
