use super::{DeserializeSpan, ResultSpan, SerializeSpan};
use crate::consumer::span as consumer;
use core::marker::PhantomData;
use upstream_runtime::{Env, Result};
use upstream_time::Instant;
use tracing::{Level, Span, event};

pub mod span {
    pub use super::*;
}

pub struct Unary<Req, Res> {
    span: Span,
    meta: &'static tracing::Metadata<'static>,
    clean_exit: bool,
    start: Instant,
    _r: PhantomData<fn() -> (Req, Res)>,
}

impl<Req, Res> Unary<Req, Res> {
    #[track_caller]
    pub fn new<E: Env>(env: &E, span: Span) -> Option<Self> {
        let meta = span.metadata()?;
        let start = env.now();

        let v = Unary {
            span,
            meta,
            clean_exit: false,
            start,
            _r: PhantomData,
        };

        v.event::<E>(None, "started");

        Some(v)
    }

    #[track_caller]
    fn event<E: Env>(&self, env: Option<&E>, msg: &str) {
        macro_rules! evt {
            ($level:expr) => {{
                if let Some(env) = env {
                    let elapsed = env.now().duration_since(self.start);
                    event!(parent: self.span.clone(), $level, { ?elapsed }, "{}", msg);
                } else {
                    event!(parent: self.span.clone(), $level, "{}", msg);
                }
            }};
        }
        match *self.meta.level() {
            Level::DEBUG => evt!(Level::DEBUG),
            Level::INFO => evt!(Level::INFO),
            Level::WARN => evt!(Level::WARN),
            Level::ERROR => evt!(Level::ERROR),
            Level::TRACE => evt!(Level::TRACE),
        }
    }
}

impl<Req, Res, T> crate::Span<T> for Unary<Req, Res>
where
    Req: 'static,
    Res: 'static,
{
    fn exit<E: Env>(mut self, env: &E, _result: &T) {
        self.clean_exit = true;
        self.event(Some(env), "finished");
    }
}

impl<Req, Res> Drop for Unary<Req, Res> {
    fn drop(&mut self) {
        if !self.clean_exit {
            event!(parent: self.span.clone(), Level::WARN, "unexpected exit");
        }
    }
}

impl<Req, Res> consumer::Unary<Req, Res> for Unary<Req, Res>
where
    Req: 'static,
    Res: 'static,
{
    fn on_deserialize<E: Env>(&mut self, env: &E, size: usize) -> impl crate::Span<Result<Res>> {
        let span = self.span.clone();

        let start = if span.is_disabled() {
            None
        } else {
            Some(env.now())
        };

        Some(DeserializeSpan { span, size, start })
    }

    fn on_serialize<E: Env>(&mut self, env: &E, _request: &Req) -> impl crate::Span<usize> {
        let span = self.span.clone();

        let start = if span.is_disabled() {
            None
        } else {
            Some(env.now())
        };

        SerializeSpan { span, start }
    }

    fn on_serialize_multicast<E: Env>(&mut self, env: &E, len: usize) {
        let _ = env;
        let _ = len;
    }

    fn on_execute<E: Env>(
        &mut self,
        env: &E,
        _request_size: usize,
    ) -> impl crate::Span<Result<usize>> {
        let span = self.span.clone();

        let start = if span.is_disabled() {
            None
        } else {
            Some(env.now())
        };

        ResultSpan {
            span,
            start,
            label: "executed",
        }
    }
}
