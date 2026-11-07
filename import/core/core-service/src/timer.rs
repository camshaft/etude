use std::mem::MaybeUninit;

use crate::Span;

pub trait Timer: Send + 'static {}

impl Timer for () {}

impl<A: Timer, B: Timer> Timer for (A, B) {}

impl<T> Timer for Option<T> where T: Timer {}

pub struct TimerSpan<E, S>
where
    E: upstream_runtime::Env,
    S: Span<()>,
{
    pub env: E,
    pub span: MaybeUninit<S>,
}

impl<E, S> Timer for TimerSpan<E, S>
where
    E: upstream_runtime::Env,
    S: Span<()>,
{
}

impl<E, S> TimerSpan<E, S>
where
    E: upstream_runtime::Env,
    S: Span<()>,
{
    pub fn new(env: E, span: S) -> Self {
        Self {
            env,
            span: MaybeUninit::new(span),
        }
    }
}

impl<E, S> Drop for TimerSpan<E, S>
where
    E: upstream_runtime::Env,
    S: Span<()>,
{
    fn drop(&mut self) {
        let env = &self.env;
        let span = unsafe { self.span.assume_init_read() };
        span.exit(env, &());
    }
}
