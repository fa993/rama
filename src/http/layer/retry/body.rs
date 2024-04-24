use bytes::Bytes;

#[derive(Debug, Clone)]
/// A body that can be clone and used for requests that have to be rertried.
pub struct RetryBody {
    bytes: Option<Bytes>,
}

impl RetryBody {
    pub(crate) fn new(bytes: Bytes) -> Self {
        RetryBody { bytes: Some(bytes) }
    }

    /// Turn this body into bytes.
    pub fn into_bytes(self) -> Option<Bytes> {
        self.bytes
    }
}

impl crate::http::dep::http_body::Body for RetryBody {
    type Data = Bytes;
    type Error = crate::error::BoxError;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        std::task::Poll::Ready(
            self.bytes
                .take()
                .map(|bytes| Ok(http_body::Frame::data(bytes))),
        )
    }

    fn is_end_stream(&self) -> bool {
        true
    }
}

impl From<RetryBody> for crate::http::Body {
    fn from(body: RetryBody) -> Self {
        match body.bytes {
            Some(bytes) => bytes.into(),
            None => crate::http::Body::empty(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::BodyExtractExt;

    #[tokio::test]
    async fn consume_retry_body() {
        let body = RetryBody::new(Bytes::from("hello"));
        let s = body.try_into_string().await.unwrap();
        assert_eq!(s, "hello");
    }
}
