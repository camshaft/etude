// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use super::{
    Acceptor, Request, RequestHeader, RequestProvider, Response, ResponseHeader, ResponseProvider,
    span,
};
use crate::{Either, RequestStream, ResponseStream, Service, Span};
use core::marker::PhantomData;
use futures::StreamExt as _;
use upstream_bytevec::{self as bytevec, ByteVec};
use upstream_runtime::{Env, Error};
use upstream_wire::{Deserialize, DeserializeError, Serialize, Wire};

/// Macro to generate request and response provider adapters
macro_rules! impl_provider_adapter {
    ($adapter:ident, $target:ident, $other:ident) => {
        pub struct $adapter<Req, Other> {
            inner: Req,
            other: PhantomData<fn() -> Other>,
        }

        impl<Req, Other> $adapter<Req, Other> {
            fn new(inner: Req) -> Self {
                Self {
                    inner,
                    other: PhantomData,
                }
            }
        }

        impl<Req, Other> Deserialize for $adapter<Req, Other>
        where
            Req: Deserialize,
        {
            fn deserialize(bytes: &mut ByteVec) -> Result<Self, DeserializeError> {
                let inner = Req::deserialize(bytes)?;
                Ok(Self::new(inner))
            }
        }

        impl<Req, Other, Left, Right> TryFrom<Either<Left, Right>> for $adapter<Req, Other>
        where
            Req: TryFrom<$target, Error = Error> + Wire,
        {
            type Error = Error;

            fn try_from(value: Either<Left, Right>) -> Result<Self, Self::Error> {
                let Either::$target(value) = value else {
                    debug_assert!(false, "unexpected variant passed to service");
                    return Err(Error::Wire(DeserializeError::new(
                        upstream_wire::de::ErrorKind::InvalidVariant,
                        const { &Req::TYPE },
                    )));
                };
                let value = value.try_into()?;
                Ok(Self::new(value))
            }
        }

        impl<Req, Other> Wire for $adapter<Req, Other>
        where
            Req: Wire,
        {
            const TYPE: upstream_wire::Type = Req::TYPE;
        }

        impl<Res, Other> Serialize for $adapter<Res, Other>
        where
            Res: Serialize,
        {
            fn serialize<B: bytevec::Behavior>(self, out: &mut bytevec::Builder<B>) {
                self.inner.serialize(out)
            }
        }

        impl<Res, Other, $target, $other> From<$adapter<Res, Other>> for Either<Left, Right>
        where
            Res: 'static + Serialize + Into<$target> + Send,
        {
            fn from(value: $adapter<Res, Other>) -> Self {
                Self::$target(value.inner.into())
            }
        }

        impl<Target, Other, T> Span<T> for $adapter<Target, Other>
        where
            Target: Span<T>,
            Other: 'static,
        {
            fn exit<E: Env>(self, env: &E, value: &T) {
                self.inner.exit(env, value);
            }
        }

        impl<Target, Other, T> span::FullRequest<T> for $adapter<Target, Other>
        where
            Target: span::FullRequest<T>,
            Other: 'static,
        {
            fn on_receive<E: Env>(&mut self, env: &E) -> impl crate::Span<Result<usize, Error>> {
                self.inner.on_receive(env)
            }

            fn on_deserialize<E: Env>(
                &mut self,
                env: &E,
                payload_size: usize,
            ) -> impl crate::Span<Result<T, Error>> {
                self.inner.on_deserialize(env, payload_size)
            }
        }

        impl<Target, Other, T> span::FullResponse<T> for $adapter<Target, Other>
        where
            Target: span::FullResponse<T>,
            Other: 'static,
        {
        }

        impl<Req, $target, $other> RequestProvider<$target> for $adapter<Req, $other>
        where
            Req: RequestProvider<(Left, Right)>,
            $target: Service,
            $other: Service,
        {
            async fn read<E, ReqT, Span>(self, env: &E, span: &mut Span) -> Result<ReqT, Error>
            where
                E: Env,
                ReqT: Request<$target>,
                Span: super::span::FullRequest<ReqT>,
            {
                // TODO
                let _ = span;
                let mut span = ();
                let result = self.inner.read(env, &mut span).await?;
                let result: $adapter<ReqT, $other::Request> = result;
                let result = result.inner;
                Ok(result)
            }

            async fn read_with_header<E, ReqH, ReqT, Span>(
                self,
                env: &E,
                span: &mut Span,
            ) -> Result<(ReqH, ReqT), Error>
            where
                E: Env,
                ReqH: RequestHeader<$target>,
                ReqT: Request<$target>,
                Span: super::span::FullRequest<(ReqH, ReqT)>,
            {
                // TODO
                let _ = span;
                let mut span = ();
                let (header, request) = self.inner.read_with_header(env, &mut span).await?;
                let header: $adapter<ReqH, ()> = header;
                let header = header.inner;
                let request: $adapter<ReqT, ()> = request;
                let request = request.inner;
                Ok((header, request))
            }

            async fn read_stream<E, ReqT, I>(
                self,
                env: &E,
                instrument: I,
            ) -> Result<impl RequestStream<ReqT>, Error>
            where
                E: Env,
                ReqT: Request<$target>,
                I: super::span::StreamingRequest<ReqT> + 'static,
            {
                // Composed services don't instrument per-frame; drop it.
                let _ = instrument;
                let stream = self.inner.read_stream(env, ()).await?;
                let stream = stream.map(|item| item.map(|item: $adapter<ReqT, ()>| item.inner));
                Ok(stream)
            }

            async fn read_stream_with_header<E, H, ReqT, I>(
                self,
                env: &E,
                instrument: I,
            ) -> Result<(H, impl RequestStream<ReqT>), Error>
            where
                E: Env,
                H: RequestHeader<$target>,
                ReqT: Request<$target>,
                I: super::span::StreamingRequest<ReqT> + 'static,
            {
                let _ = instrument;
                let (header, stream) = self.inner.read_stream_with_header(env, ()).await?;
                let header: $adapter<H, $other::RequestHeader> = header;
                let header = header.inner;

                let stream = stream.map(|item| item.map(|item: $adapter<ReqT, ()>| item.inner));

                Ok((header, stream))
            }
        }

        impl<Res, $target, $other> ResponseProvider<$target> for $adapter<Res, $other>
        where
            Res: ResponseProvider<(Left, Right)>,
            $target: Service,
            $other: Service,
        {
            async fn finish_unidirectional<E, Span>(self, env: &E, span: Span)
            where
                E: Env,
                Span: crate::Span<Result<(), Error>>,
            {
                self.inner.finish_unidirectional(env, span).await
            }

            async fn write<E, ResT, Span>(self, env: &E, response: ResT, span: Span)
            where
                E: Env,
                ResT: Response<$target>,
                Span: super::span::FullResponse<ResT>,
            {
                let response = <$adapter<ResT, ()>>::new(response);
                // TODO
                let _ = span;
                let span = ();
                self.inner.write(env, response, span).await
            }

            async fn write_with_header<E, ResH, ResT, Span>(
                self,
                env: &E,
                header: ResH,
                response: ResT,
                span: Span,
            ) where
                E: Env,
                ResH: ResponseHeader<$target>,
                ResT: Response<$target>,
                Span: super::span::FullResponse<(ResH, ResT)>,
            {
                let header = <$adapter<ResH, ()>>::new(header);
                let response = <$adapter<ResT, ()>>::new(response);
                // TODO
                let _ = span;
                let span = ();
                self.inner
                    .write_with_header(env, header, response, span)
                    .await;
            }

            async fn write_raw<E, ResT, Span>(self, env: &E, response: ResT, span: Span)
            where
                E: Env,
                ResT: upstream_buffer::reader::storage::Infallible + Send,
                Span: super::span::FullResponse<usize>,
            {
                self.inner.write_raw(env, response, span).await
            }

            async fn write_stream<E, ResT, Sp>(
                self,
                env: &E,
                response_stream: impl ResponseStream<ResT>,
                span: Sp,
            ) where
                E: Env,
                ResT: Response<$target>,
                Sp: super::span::StreamingResponse<ResT>,
            {
                // Composed services don't instrument per-frame; drop the span.
                let _ = span;
                let response_stream =
                    response_stream.map(|item| item.map(<$adapter<ResT, ()>>::new));
                self.inner.write_stream(env, response_stream, ()).await
            }

            async fn write_stream_with_header<E, ResH, ResT, Sp>(
                self,
                env: &E,
                header: ResH,
                response_stream: impl ResponseStream<ResT>,
                span: Sp,
            ) where
                E: Env,
                ResH: ResponseHeader<$target>,
                ResT: Response<$target>,
                Sp: super::span::StreamingResponse<ResT>,
            {
                let _ = span;
                let header = <$adapter<ResH, ()>>::new(header);
                let response_stream =
                    response_stream.map(|item| item.map(<$adapter<ResT, ()>>::new));
                self.inner
                    .write_stream_with_header(env, header, response_stream, ())
                    .await
            }

            async fn write_error<E, Sp>(self, env: &E, err: Error, span: Sp)
            where
                E: Env,
                Sp: crate::Span<Result<(), Error>>,
            {
                self.inner.write_error(env, err, span).await
            }

            async fn write_stream_header_error<E, Sp>(self, env: &E, err: Error, span: Sp)
            where
                E: Env,
                Sp: crate::Span<Result<(), Error>>,
            {
                self.inner.write_stream_header_error(env, err, span).await
            }

            async fn write_raw_error<E, Sp>(self, env: &E, err: Error, span: Sp)
            where
                E: Env,
                Sp: crate::Span<Result<(), Error>>,
            {
                self.inner.write_raw_error(env, err, span).await
            }
        }
    };
}

// Generate left and right adapters
impl_provider_adapter!(LeftAdapter, Left, Right);
impl_provider_adapter!(RightAdapter, Right, Left);

impl<A, B, E, PeerAddr> Acceptor<E, PeerAddr> for (A, B)
where
    A: Acceptor<E, PeerAddr>,
    B: Acceptor<E, PeerAddr>,
    E: Env,
    PeerAddr: 'static + Send + Sync,
{
    type Service = (A::Service, B::Service);

    async fn dispatch<Req, Res>(
        &self,
        env: &E,
        peer_addr: &PeerAddr,
        method: <Self::Service as Service>::Methods,
        request: Req,
        response: Res,
    ) where
        Req: RequestProvider<Self::Service>,
        Res: ResponseProvider<Self::Service>,
    {
        match method {
            Either::Left(method) => {
                let request = LeftAdapter::new(request);
                let response = LeftAdapter::new(response);
                self.0
                    .dispatch(env, peer_addr, method, request, response)
                    .await
            }
            Either::Right(method) => {
                let request = RightAdapter::new(request);
                let response = RightAdapter::new(response);
                self.1
                    .dispatch(env, peer_addr, method, request, response)
                    .await
            }
        }
    }

    async fn while_open(&self) {
        // return when either closes
        use core::pin::pin;
        let a = pin!(self.0.while_open());
        let b = pin!(self.1.while_open());
        futures::future::select(a, b).await;
    }
}
