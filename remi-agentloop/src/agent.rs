use std::future::Future;
use futures::Stream;

/// Core Agent trait — async streaming, fully generic, no Send bound
pub trait Agent {
    type Request;
    type Response;
    type Error;

    fn chat(
        &self,
        req: Self::Request,
    ) -> impl Future<Output = Result<impl Stream<Item = Self::Response>, Self::Error>>;
}

/// Extension methods auto-provided to all `Agent` impls via blanket impl
pub trait AgentExt: Agent + Sized {
    /// Map each Response item in the stream
    fn map_response<F, R>(self, f: F) -> crate::adapters::map::MapResponse<Self, F>
    where
        F: Fn(Self::Response) -> R,
    {
        crate::adapters::map::MapResponse { inner: self, f }
    }

    /// Transform the Request type before forwarding
    fn map_request<F, NewReq>(self, f: F) -> crate::adapters::map::MapRequest<Self, F, NewReq>
    where
        F: Fn(NewReq) -> Self::Request,
    {
        crate::adapters::map::MapRequest { inner: self, f, _phantom: std::marker::PhantomData }
    }

    /// Transform the Error type
    fn map_err<F, NewErr>(self, f: F) -> crate::adapters::map::MapErr<Self, F>
    where
        F: Fn(Self::Error) -> NewErr,
    {
        crate::adapters::map::MapErr { inner: self, f }
    }

    /// Apply a Layer adapter
    fn layer<L: Layer<Self>>(self, layer: L) -> L::Output {
        layer.layer(self)
    }

    /// Type-erase into BoxedAgent — opt-in, user-explicit
    fn boxed(self) -> crate::adapters::boxed::BoxedAgent<Self::Request, Self::Response, Self::Error>
    where
        Self: Send + Sync + 'static,
        Self::Request: 'static,
        Self::Response: 'static,
        Self::Error: 'static,
    {
        crate::adapters::boxed::BoxedAgent::new(self)
    }
}

impl<A: Agent> AgentExt for A {}

/// Layer wraps one Agent into another
pub trait Layer<A: Agent> {
    type Output: Agent;
    fn layer(self, inner: A) -> Self::Output;
}
