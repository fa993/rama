use crate::http::{RequestContext, Version};
use base64::Engine;
use serde::Deserialize;
use std::{future::Future, str::FromStr};

mod internal;
pub use internal::{Proxy, ProxyCsvRowReader, ProxyCsvRowReaderError, ProxyCsvRowReaderErrorKind};

mod str;
#[doc(inline)]
pub use str::StringFilter;

const BASE64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

#[derive(Debug, Clone, PartialEq, Eq)]
/// The credentials to use to authenticate with the proxy.
pub enum ProxyCredentials {
    /// Basic authentication
    ///
    /// See <https://datatracker.ietf.org/doc/html/rfc7617> for more information.
    Basic {
        /// The username to use to authenticate with the proxy.
        username: String,
        /// The optional password to use to authenticate with the proxy,
        /// in combination with the username.
        password: Option<String>,
    },
    /// Bearer token authentication, token content is opaque for the proxy facilities.
    ///
    /// See <https://datatracker.ietf.org/doc/html/rfc6750> for more information.
    Bearer(String),
}

impl ProxyCredentials {
    /// Get the username from the credentials, if any.
    pub fn username(&self) -> Option<&str> {
        match self {
            ProxyCredentials::Basic { username, .. } => Some(username),
            ProxyCredentials::Bearer(_) => None,
        }
    }

    /// Get the password from the credentials, if any.
    pub fn password(&self) -> Option<&str> {
        match self {
            ProxyCredentials::Basic { password, .. } => password.as_deref(),
            ProxyCredentials::Bearer(_) => None,
        }
    }

    /// Get the bearer token from the credentials, if any.
    pub fn bearer(&self) -> Option<&str> {
        match self {
            ProxyCredentials::Bearer(token) => Some(token),
            ProxyCredentials::Basic { .. } => None,
        }
    }
}

impl std::fmt::Display for ProxyCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProxyCredentials::Basic { username, password } => match password {
                Some(password) => write!(
                    f,
                    "Basic {}",
                    BASE64.encode(format!("{}:{}", username, password))
                ),
                None => write!(f, "Basic {}", BASE64.encode(username)),
            },
            ProxyCredentials::Bearer(token) => write!(f, "Bearer {}", token),
        }
    }
}

#[derive(Debug)]
/// The error that can be returned when parsing a proxy credentials string.
#[non_exhaustive]
pub struct InvalidProxyCredentialsString;

impl FromStr for ProxyCredentials {
    type Err = InvalidProxyCredentialsString;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.splitn(2, ' ');

        match parts.next() {
            Some("Basic") => {
                let encoded = parts.next().ok_or(InvalidProxyCredentialsString)?;
                let decoded = BASE64
                    .decode(encoded)
                    .map_err(|_| InvalidProxyCredentialsString)?;
                let decoded =
                    String::from_utf8(decoded).map_err(|_| InvalidProxyCredentialsString)?;
                let mut parts = decoded.splitn(2, ':');

                let username = parts
                    .next()
                    .ok_or(InvalidProxyCredentialsString)?
                    .to_owned();
                let password = parts.next().map(str::to_owned);

                Ok(ProxyCredentials::Basic { username, password })
            }
            Some("Bearer") => {
                let token = parts.next().ok_or(InvalidProxyCredentialsString)?;
                Ok(ProxyCredentials::Bearer(token.to_owned()))
            }
            _ => Err(InvalidProxyCredentialsString),
        }
    }
}

impl std::fmt::Display for InvalidProxyCredentialsString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Invalid proxy credentials string")
    }
}

impl std::error::Error for InvalidProxyCredentialsString {}

#[derive(Debug, Default, Clone, Deserialize, PartialEq)]
/// Filter to select a specific kind of proxy.
///
/// If the `id` is specified the other fields are used
/// as a validator to see if the only possible matching proxy
/// matches these fields.
///
/// If the `id` is not specified, the other fields are used
/// to select a random proxy from the pool.
///
/// Filters can be combined to make combinations with special meaning.
/// E.g. `datacenter:true, residential:true` is essentially an ISP proxy.
///
/// ## Usage
///
/// - Use [`HeaderConfigLayer`] to have this proxy filter be given by the [`Request`] headers,
///   which will add the extracted and parsed [`ProxyFilter`] to the [`Context`]'s [`Extensions`].
/// - Or extract yourself from the username/token validated in the [`ProxyAuthLayer`]
///   to add it manually to the [`Context`]'s [`Extensions`].
///
/// [`HeaderConfigLayer`]: crate::http::layer::header_config::HeaderConfigLayer
/// [`Request`]: crate::http::Request
/// [`ProxyAuthLayer`]: crate::http::layer::proxy_auth::ProxyAuthLayer
/// [`Context`]: crate::service::Context
/// [`Extensions`]: crate::service::context::Extensions
pub struct ProxyFilter {
    /// The ID of the proxy to select.
    pub id: Option<String>,

    /// The ID of the pool from which to select the proxy.
    pub pool_id: Option<StringFilter>,

    /// The country of the proxy.
    pub country: Option<StringFilter>,

    /// The city of the proxy.
    pub city: Option<StringFilter>,

    /// Set explicitly to `true` to select a datacenter proxy.
    pub datacenter: Option<bool>,

    /// Set explicitly to `true` to select a residential proxy.
    pub residential: Option<bool>,

    /// Set explicitly to `true` to select a mobile proxy.
    pub mobile: Option<bool>,

    /// The mobile carrier desired.
    pub carrier: Option<StringFilter>,
}

/// The trait to implement to provide a proxy database to other facilities,
/// such as connection pools, to provide a proxy based on the given
/// [`RequestContext`] and [`ProxyFilter`].
pub trait ProxyDB: Send + Sync + 'static {
    /// The error type that can be returned by the proxy database
    ///
    /// Examples are generic I/O issues or
    /// even more common if no proxy match could be found.
    type Error;

    /// Get a [`Proxy`] based on the given [`RequestContext`] and [`ProxyFilter`],
    /// or return an error in case no [`Proxy`] could be returned.
    fn get_proxy(
        &self,
        ctx: RequestContext,
        filter: ProxyFilter,
    ) -> impl Future<Output = Result<Proxy, Self::Error>> + Send + '_;

    /// Same as [`Self::get_proxy`] but with a predicate
    /// to filter out found proxies that do not match the given predicate.
    fn get_proxy_if(
        &self,
        ctx: RequestContext,
        filter: ProxyFilter,
        predicate: impl Fn(&Proxy) -> bool + Send + Sync + 'static,
    ) -> impl Future<Output = Result<Proxy, Self::Error>> + Send + '_;
}

/// A fast in-memory ProxyDatabase that is the default choice for Rama.
#[derive(Debug)]
pub struct MemoryProxyDB {
    data: internal::ProxyDB,
}

// TODO: add proxy validation prior to creation of db!

impl MemoryProxyDB {
    /// Create a new in-memory proxy database with the given proxies.
    pub fn try_from_rows(proxies: Vec<Proxy>) -> Result<Self, MemoryProxyDBInsertError> {
        Ok(MemoryProxyDB {
            data: internal::ProxyDB::from_rows(proxies).map_err(|err| match err.kind() {
                internal::ProxyDBErrorKind::DuplicateKey => {
                    MemoryProxyDBInsertError::duplicate_key(err.into_input())
                }
            })?,
        })
    }

    /// Create a new in-memory proxy database with the given proxies from an iterator.
    pub fn try_from_iter<I>(proxies: I) -> Result<Self, MemoryProxyDBInsertError>
    where
        I: IntoIterator<Item = Proxy>,
    {
        Ok(MemoryProxyDB {
            data: internal::ProxyDB::from_iter(proxies).map_err(|err| match err.kind() {
                internal::ProxyDBErrorKind::DuplicateKey => {
                    MemoryProxyDBInsertError::duplicate_key(err.into_input())
                }
            })?,
        })
    }

    /// Return the number of proxies in the database.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Rerturns if the database is empty.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    fn query_from_filter(
        &self,
        ctx: RequestContext,
        filter: ProxyFilter,
    ) -> internal::ProxyDBQuery {
        let mut query = self.data.query();

        if let Some(pool_id) = filter.pool_id {
            query.pool_id(pool_id);
        }
        if let Some(country) = filter.country {
            query.country(country);
        }
        if let Some(city) = filter.city {
            query.city(city);
        }

        if let Some(value) = filter.datacenter {
            query.datacenter(value);
        }
        if let Some(value) = filter.residential {
            query.residential(value);
        }
        if let Some(value) = filter.mobile {
            query.mobile(value);
        }

        if ctx.http_version == Version::HTTP_3 {
            query.udp(true);
            query.socks5(true);
        } else {
            // NOTE: we do not test whether http/socks5 is supported,
            // as we assume that the proxy supports at least one of them.
            // It might be good to update venndb to also allow such variant checks...
            // For now however I think that's a safe assumption to make
            // as either way rama will not support something other then the
            // HTTP/Socks5 proxies for the time being.
            query.tcp(true);
        }

        query
    }
}

impl ProxyDB for MemoryProxyDB {
    type Error = MemoryProxyDBQueryError;

    async fn get_proxy(
        &self,
        ctx: RequestContext,
        filter: ProxyFilter,
    ) -> Result<Proxy, Self::Error> {
        match &filter.id {
            Some(id) => match self.data.get_by_id(id) {
                None => Err(MemoryProxyDBQueryError::not_found()),
                Some(proxy) => {
                    if proxy.is_match(&ctx, &filter) {
                        Ok(proxy.clone())
                    } else {
                        Err(MemoryProxyDBQueryError::mismatch())
                    }
                }
            },
            None => {
                let query = self.query_from_filter(ctx, filter);
                match query.execute().map(|result| result.any()).cloned() {
                    None => Err(MemoryProxyDBQueryError::not_found()),
                    Some(proxy) => Ok(proxy),
                }
            }
        }
    }

    async fn get_proxy_if(
        &self,
        ctx: RequestContext,
        filter: ProxyFilter,
        predicate: impl Fn(&Proxy) -> bool + Send + Sync + 'static,
    ) -> Result<Proxy, Self::Error> {
        match &filter.id {
            Some(id) => match self.data.get_by_id(id) {
                None => Err(MemoryProxyDBQueryError::not_found()),
                Some(proxy) => {
                    if proxy.is_match(&ctx, &filter) && predicate(proxy) {
                        Ok(proxy.clone())
                    } else {
                        Err(MemoryProxyDBQueryError::mismatch())
                    }
                }
            },
            None => {
                let query = self.query_from_filter(ctx, filter);
                match query
                    .execute()
                    .and_then(|result| result.filter(predicate))
                    .map(|result| result.any())
                    .cloned()
                {
                    None => Err(MemoryProxyDBQueryError::not_found()),
                    Some(proxy) => Ok(proxy),
                }
            }
        }
    }
}

/// The error type that can be returned by [`MemoryProxyDB`] when some of the proxies
/// could not be inserted due to a proxy that had a duplicate key or was invalid for some other reason.
#[derive(Debug)]
pub struct MemoryProxyDBInsertError {
    kind: MemoryProxyDBInsertErrorKind,
    proxies: Vec<Proxy>,
}

impl std::fmt::Display for MemoryProxyDBInsertError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.kind {
            MemoryProxyDBInsertErrorKind::DuplicateKey => write!(
                f,
                "A proxy with the same key already exists in the database"
            ),
            MemoryProxyDBInsertErrorKind::InvalidProxy => {
                write!(f, "A proxy in the list is invalid for some reason")
            }
        }
    }
}

impl std::error::Error for MemoryProxyDBInsertError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// The kind of error that [`MemoryProxyDBInsertError`] represents.
pub enum MemoryProxyDBInsertErrorKind {
    /// Duplicate key found in the proxies.
    DuplicateKey,
    /// Invalid proxy found in the proxies.
    ///
    /// This could be due to a proxy that is not valid for some reason.
    /// E.g. a proxy that neither supports http or socks5.
    InvalidProxy,
}

impl MemoryProxyDBInsertError {
    fn duplicate_key(proxies: Vec<Proxy>) -> Self {
        MemoryProxyDBInsertError {
            kind: MemoryProxyDBInsertErrorKind::DuplicateKey,
            proxies,
        }
    }

    // TOOD: enable
    // fn invalid_proxy(proxies: Vec<Proxy>) -> Self {
    //     MemoryProxyDBInsertError {
    //         kind: MemoryProxyDBInsertErrorKind::InvalidProxy,
    //         proxies,
    //     }
    // }

    /// Returns the kind of error that [`MemoryProxyDBInsertError`] represents.
    pub fn kind(&self) -> MemoryProxyDBInsertErrorKind {
        self.kind
    }

    /// Returns the proxies that were not inserted.
    pub fn proxies(&self) -> &[Proxy] {
        &self.proxies
    }

    /// Consumes the error and returns the proxies that were not inserted.
    pub fn into_proxies(self) -> Vec<Proxy> {
        self.proxies
    }
}

/// The error type that can be returned by [`MemoryProxyDB`] when no proxy could be returned.
#[derive(Debug)]
pub struct MemoryProxyDBQueryError {
    kind: MemoryProxyDBQueryErrorKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// The kind of error that [`MemoryProxyDBQueryError`] represents.
pub enum MemoryProxyDBQueryErrorKind {
    /// No proxy match could be found.
    NotFound,
    /// A proxy looked up by key had a config that did not match the given filters/requirements.
    Mismatch,
}

impl std::fmt::Display for MemoryProxyDBQueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.kind {
            MemoryProxyDBQueryErrorKind::NotFound => write!(f, "No proxy match could be found"),
            MemoryProxyDBQueryErrorKind::Mismatch => write!(
                f,
                "Proxy config did not match the given filters/requirements"
            ),
        }
    }
}

impl std::error::Error for MemoryProxyDBQueryError {}

impl MemoryProxyDBQueryError {
    fn not_found() -> Self {
        MemoryProxyDBQueryError {
            kind: MemoryProxyDBQueryErrorKind::NotFound,
        }
    }

    fn mismatch() -> Self {
        MemoryProxyDBQueryError {
            kind: MemoryProxyDBQueryErrorKind::Mismatch,
        }
    }

    /// Returns the kind of error that [`MemoryProxyDBQueryError`] represents.
    pub fn kind(&self) -> MemoryProxyDBQueryErrorKind {
        self.kind
    }
}

#[cfg(test)]
mod tests {
    use itertools::Itertools;

    use super::*;

    #[test]
    fn test_proxy_credentials_from_str_basic() {
        let credentials: ProxyCredentials = "Basic dXNlcm5hbWU6cGFzc3dvcmQ=".parse().unwrap();
        assert_eq!(credentials.username().unwrap(), "username");
        assert_eq!(credentials.password().unwrap(), "password");
    }

    #[test]
    fn test_proxy_credentials_from_str_bearer() {
        let credentials: ProxyCredentials = "Bearer bar".parse().unwrap();
        assert_eq!(credentials.bearer().unwrap(), "bar");
    }

    #[test]
    fn test_proxy_credentials_from_str_invalid() {
        let credentials: Result<ProxyCredentials, _> = "Invalid".parse();
        assert!(credentials.is_err());
    }

    #[test]
    fn test_proxy_credentials_display_basic() {
        let credentials = ProxyCredentials::Basic {
            username: "username".to_owned(),
            password: Some("password".to_owned()),
        };
        assert_eq!(credentials.to_string(), "Basic dXNlcm5hbWU6cGFzc3dvcmQ=");
    }

    #[test]
    fn test_proxy_credentials_display_basic_no_password() {
        let credentials = ProxyCredentials::Basic {
            username: "username".to_owned(),
            password: None,
        };
        assert_eq!(credentials.to_string(), "Basic dXNlcm5hbWU=");
    }

    #[test]
    fn test_proxy_credentials_display_bearer() {
        let credentials = ProxyCredentials::Bearer("foo".to_owned());
        assert_eq!(credentials.to_string(), "Bearer foo");
    }

    const RAW_CSV_DATA: &str = include_str!("./test_proxydb_rows.csv");

    async fn memproxydb() -> MemoryProxyDB {
        let mut reader = ProxyCsvRowReader::raw(RAW_CSV_DATA);
        let mut rows = Vec::new();
        while let Some(proxy) = reader.next().await.unwrap() {
            rows.push(proxy);
        }
        MemoryProxyDB::try_from_rows(rows).unwrap()
    }

    #[tokio::test]
    async fn test_load_memproxydb_from_rows() {
        let db = memproxydb().await;
        assert_eq!(db.len(), 64);
    }

    fn h2_req_context() -> RequestContext {
        RequestContext {
            http_version: Version::HTTP_2,
            scheme: crate::uri::Scheme::Https,
            host: Some("example.com".to_owned()),
            port: None,
        }
    }

    #[tokio::test]
    async fn test_memproxydb_get_proxy_by_id_found() {
        let db = memproxydb().await;
        let ctx = h2_req_context();
        let filter = ProxyFilter {
            id: Some("1549558402".to_owned()),
            ..Default::default()
        };
        let proxy = db.get_proxy(ctx, filter).await.unwrap();
        assert_eq!(proxy.id, "1549558402");
    }

    #[tokio::test]
    async fn test_memproxydb_get_proxy_by_id_found_correct_filters() {
        let db = memproxydb().await;
        let ctx = h2_req_context();
        let filter = ProxyFilter {
            id: Some("1549558402".to_owned()),
            pool_id: Some(StringFilter::new("poolA")),
            country: Some(StringFilter::new("AU")),
            city: Some(StringFilter::new("Adelaide")),
            datacenter: Some(false),
            residential: Some(false),
            mobile: Some(true),
            carrier: Some(StringFilter::new("AT&T")),
        };
        let proxy = db.get_proxy(ctx, filter).await.unwrap();
        assert_eq!(proxy.id, "1549558402");
    }

    #[tokio::test]
    async fn test_memproxydb_get_proxy_by_id_not_found() {
        let db = memproxydb().await;
        let ctx = h2_req_context();
        let filter = ProxyFilter {
            id: Some("notfound".to_owned()),
            ..Default::default()
        };
        let err = db.get_proxy(ctx, filter).await.unwrap_err();
        assert_eq!(err.kind(), MemoryProxyDBQueryErrorKind::NotFound);
    }

    #[tokio::test]
    async fn test_memproxydb_get_proxy_by_id_mismatch_filter() {
        let db = memproxydb().await;
        let ctx = h2_req_context();
        let filters = [
            ProxyFilter {
                id: Some("1549558402".to_owned()),
                pool_id: Some(StringFilter::new("poolB")),
                ..Default::default()
            },
            ProxyFilter {
                id: Some("1549558402".to_owned()),
                country: Some(StringFilter::new("US")),
                ..Default::default()
            },
            ProxyFilter {
                id: Some("1549558402".to_owned()),
                city: Some(StringFilter::new("New York")),
                ..Default::default()
            },
            ProxyFilter {
                id: Some("1549558402".to_owned()),
                datacenter: Some(true),
                ..Default::default()
            },
            ProxyFilter {
                id: Some("1549558402".to_owned()),
                residential: Some(true),
                ..Default::default()
            },
            ProxyFilter {
                id: Some("1549558402".to_owned()),
                mobile: Some(false),
                ..Default::default()
            },
            ProxyFilter {
                id: Some("1549558402".to_owned()),
                carrier: Some(StringFilter::new("Verizon")),
                ..Default::default()
            },
        ];
        for filter in filters.iter() {
            let err = db.get_proxy(ctx.clone(), filter.clone()).await.unwrap_err();
            assert_eq!(err.kind(), MemoryProxyDBQueryErrorKind::Mismatch);
        }
    }

    fn h3_req_context() -> RequestContext {
        RequestContext {
            http_version: Version::HTTP_3,
            scheme: crate::uri::Scheme::Https,
            host: Some("example.com".to_owned()),
            port: Some(8443),
        }
    }

    #[tokio::test]
    async fn test_memproxydb_get_proxy_by_id_mismatch_req_context() {
        let db = memproxydb().await;
        let ctx = h3_req_context();
        let filter = ProxyFilter {
            id: Some("1549558402".to_owned()),
            ..Default::default()
        };
        // this proxy does not support socks5 UDP, which is what we need
        let err = db.get_proxy(ctx, filter).await.unwrap_err();
        assert_eq!(err.kind(), MemoryProxyDBQueryErrorKind::Mismatch);
    }

    #[tokio::test]
    async fn test_memorydb_get_h3_capable_proxies() {
        let db = memproxydb().await;
        let ctx = h3_req_context();
        let filter = ProxyFilter::default();
        let mut found_ids = Vec::new();
        for _ in 0..5000 {
            let proxy = db.get_proxy(ctx.clone(), filter.clone()).await.unwrap();
            if found_ids.contains(&proxy.id) {
                continue;
            }
            assert!(proxy.udp);
            assert!(proxy.socks5);
            found_ids.push(proxy.id);
        }
        assert_eq!(found_ids.len(), 14);
        assert_eq!(
            found_ids.iter().sorted().join(","),
            r##"1333564166,2012271852,2432027317,2503805829,2800824798,2862707252,2865590509,3012515011,3439682932,3813409672,3904077149,4064485987,777999237,878701584"##
        );
    }

    #[tokio::test]
    async fn test_memorydb_get_h2_capable_proxies() {
        let db = memproxydb().await;
        let ctx = h2_req_context();
        let filter = ProxyFilter::default();
        let mut found_ids = Vec::new();
        for _ in 0..5000 {
            let proxy = db.get_proxy(ctx.clone(), filter.clone()).await.unwrap();
            if found_ids.contains(&proxy.id) {
                continue;
            }
            assert!(proxy.tcp);
            found_ids.push(proxy.id);
        }
        assert_eq!(found_ids.len(), 30);
        assert_eq!(
            found_ids.iter().sorted().join(","),
            r#"1043547900,1333564166,1393984890,1549558402,1629940602,17693162,2012271852,2339597854,2436687663,2503805829,2503885092,260229916,2692540368,295238804,2998884635,3012515011,3400641131,35672966,3813409672,3904077149,3916451868,393695089,4064485987,4076081397,4077606290,4157991939,838438595,878701584,913889340,915185154"#,
        );
    }

    #[tokio::test]
    async fn test_memorydb_get_any_country_proxies() {
        let db = memproxydb().await;
        let ctx = h2_req_context();
        let filter = ProxyFilter {
            // there are no explicit BE proxies,
            // so these will only match the proxies that have a wildcard country
            country: Some("BE".into()),
            ..Default::default()
        };
        let mut found_ids = Vec::new();
        for _ in 0..5000 {
            let proxy = db.get_proxy(ctx.clone(), filter.clone()).await.unwrap();
            if found_ids.contains(&proxy.id) {
                continue;
            }
            found_ids.push(proxy.id);
        }
        assert_eq!(found_ids.len(), 5);
        assert_eq!(
            found_ids.iter().sorted().join(","),
            r#"2012271852,2436687663,2503885092,260229916,35672966"#,
        );
    }

    #[tokio::test]
    async fn test_memorydb_get_h3_capable_mobile_residential_be_asterix_proxies() {
        let db = memproxydb().await;
        let ctx = h3_req_context();
        let filter = ProxyFilter {
            country: Some("BE".into()),
            mobile: Some(true),
            residential: Some(true),
            ..Default::default()
        };
        for _ in 0..50 {
            let proxy = db.get_proxy(ctx.clone(), filter.clone()).await.unwrap();
            assert_eq!(proxy.id, "2012271852");
        }
    }
}
