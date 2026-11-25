use std::{fmt, future::Future, time::Duration};

use chrono::NaiveDate;
use rand::RngExt;
use serde_json::Value;
use url::Url;
use uuid::Uuid;

use crate::generated::{self, ApiOpError, HttpClient, HttpError};

const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36";
const MAX_ATTEMPTS: u8 = 3;
pub const DEFAULT_PAGE_SIZE: i64 = 50;
const OMIT: Option<&str> = None;

/// SDK errors do not retain response bodies, credentials or request URLs.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("expected an HTTPS API origin without credentials, path, query or fragment")]
    InvalidOrigin,
    #[error(
        "expected an HTTP mock origin on a loopback IP without credentials, path, query or fragment"
    )]
    InvalidMockOrigin,
    #[error("page and page size must be positive")]
    InvalidPaging,
    #[error("invalid bearer token")]
    InvalidToken,
    #[error("Curious request failed at the transport or body-read layer")]
    Transport,
    #[error("Curious response exceeded {limit} bytes")]
    ResponseTooLarge { limit: usize },
    #[error("Curious returned an invalid response shape")]
    ResponseShape,
    #[error("Curious returned HTTP {status} after {attempts} attempt(s)")]
    HttpStatus { status: u16, attempts: u8 },
    #[error("applet was not returned by Curious")]
    AppletNotFound,
    #[error("applet encryption metadata is missing")]
    MissingEncryption,
    #[error("export and encryption metadata belong to different applets or origins")]
    MismatchedApplet,
    #[error("invalid applet encryption modulus")]
    InvalidModulus,
}

/// Page selection. Defaults to 50 items. Larger pages are allowed but are more
/// likely to hit Curious gateway timeouts; try smaller pages if timeouts persist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Paging {
    page: i64,
    size: i64,
}

impl Default for Paging {
    fn default() -> Self {
        Self {
            page: 1,
            size: DEFAULT_PAGE_SIZE,
        }
    }
}

impl Paging {
    pub fn new(page: i64, page_size: i64) -> Result<Self, Error> {
        if page <= 0 || page_size <= 0 {
            return Err(Error::InvalidPaging);
        }
        Ok(Self {
            page,
            size: page_size,
        })
    }
}

/// A session value. Login returns a new client instead of changing this one.
#[derive(Clone)]
pub struct Client {
    origin: String,
    anonymous: HttpClient,
    reads: HttpClient,
}

#[derive(Debug, Clone)]
pub enum Login {
    Authenticated(Client),
    MfaRequired(MfaChallenge),
}

#[derive(Clone)]
pub struct MfaChallenge {
    token: String,
    session_id: String,
    user_id: String,
    user_email: String,
}

impl MfaChallenge {
    pub fn token(&self) -> &str {
        &self.token
    }
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    pub fn user_id(&self) -> &str {
        &self.user_id
    }
    pub fn user_email(&self) -> &str {
        &self.user_email
    }
}

#[derive(Clone)]
pub struct Applet {
    pub(crate) origin: String,
    pub(crate) id: Uuid,
    name: String,
    metadata: Value,
}

impl Applet {
    pub fn id(&self) -> Uuid {
        self.id
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn metadata(&self) -> &Value {
        &self.metadata
    }

    pub(crate) fn encryption(&self) -> Result<(&str, &str), Error> {
        let context = &self.metadata["encryption"];
        context["prime"]
            .as_str()
            .zip(context["accountId"].as_str())
            .ok_or(Error::MissingEncryption)
    }
}

#[derive(Debug, Clone)]
pub struct AppletPage {
    applets: Vec<Applet>,
    count: Option<i64>,
}

impl AppletPage {
    pub fn applets(&self) -> &[Applet] {
        &self.applets
    }
    /// Server-provided count. Not guaranteed to be a total.
    pub fn count(&self) -> Option<i64> {
        self.count
    }
}

#[derive(Clone)]
pub struct ExportPage {
    pub(crate) origin: String,
    pub(crate) applet_id: Uuid,
    pub(crate) answers: Vec<Value>,
    pub(crate) activities: Vec<Value>,
}

impl ExportPage {
    pub fn answers(&self) -> &[Value] {
        &self.answers
    }
    pub fn activities(&self) -> &[Value] {
        &self.activities
    }
}

// None of these values should expose data in incidental logging.
macro_rules! redacted_debug {
    ($($type:ident),+ $(,)?) => {$(
        impl fmt::Debug for $type {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct(stringify!($type)).finish_non_exhaustive()
            }
        }
    )+};
}
pub(crate) use redacted_debug;
redacted_debug!(Client, MfaChallenge, Applet, ExportPage);

impl Client {
    pub fn new(base_url: &str, bearer_token: Option<&str>) -> Result<Self, Error> {
        let origin = origin(base_url)?;
        Self::build(origin, bearer_token, None, false)
    }

    /// Connect to an HTTP Prism mock on a loopback IP. Use synthetic credentials only.
    /// `prefer` is an optional Prism response preference, e.g. `code=401`.
    pub fn for_mock(
        base_url: &str,
        bearer_token: Option<&str>,
        prefer: Option<&str>,
    ) -> Result<Self, Error> {
        let url = parse_origin(base_url).map_err(|_| Error::InvalidMockOrigin)?;
        let loopback = match url.host() {
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            _ => false,
        };
        if url.scheme() != "http" || !loopback {
            return Err(Error::InvalidMockOrigin);
        }
        Self::build(
            url.origin().ascii_serialization(),
            bearer_token,
            prefer,
            true,
        )
    }

    fn build(
        origin: String,
        bearer_token: Option<&str>,
        prefer: Option<&str>,
        mock: bool,
    ) -> Result<Self, Error> {
        let mut builder = reqwest::Client::builder()
            .https_only(!mock)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(Duration::from_secs(30))
            .user_agent(USER_AGENT);
        if mock {
            builder = builder.no_proxy();
        }
        let transport = builder.build().map_err(|_| Error::Transport)?;
        let mut anonymous =
            HttpClient::with_client_and_config(transport, false).with_base_url(origin.clone());
        if let Some(prefer) = prefer {
            anonymous = anonymous.with_header("Prefer", prefer);
        }
        let reads = with_token(&anonymous, bearer_token)?;
        Ok(Self {
            origin,
            anonymous,
            reads,
        })
    }

    pub async fn login(&self, email: &str, password: &str) -> Result<Login, Error> {
        let response = self
            .anonymous
            .get_token_auth_login_post(
                OMIT,
                OMIT,
                OMIT,
                generated::UserLoginRequest::new(email.into(), password.into()),
            )
            .await;
        // For a 200 decoding error the generated client already holds the body.
        // Decode it privately to support CTK's minimal token-only envelope.
        let value = match response {
            Ok(result) => model_value(result)?,
            Err(ApiOpError::Api(error)) if error.status == 200 => {
                serde_json::from_slice(&error.raw_body).map_err(|_| Error::ResponseShape)?
            }
            Err(error) => return Err(operation_error(error, 1)),
        };
        self.login_value(&value)
    }

    fn login_value(&self, value: &Value) -> Result<Login, Error> {
        let result = value
            .get("result")
            .filter(|v| v.is_object())
            .ok_or(Error::ResponseShape)?;
        let challenge_fields = ["mfaToken", "mfaSessionId", "userId", "userEmail"];
        if !result["mfaRequired"].is_null() && !result["mfaRequired"].is_boolean() {
            return Err(Error::ResponseShape);
        }
        let mfa = result["mfaRequired"] == true
            || challenge_fields
                .iter()
                .any(|field| !result[field].is_null());
        let token = &result["token"];
        if !token.is_null() {
            if mfa {
                return Err(Error::ResponseShape);
            }
            let access = token["accessToken"]
                .as_str()
                .filter(|v| !v.is_empty())
                .ok_or(Error::ResponseShape)?;
            return Ok(Login::Authenticated(Self {
                reads: with_token(&self.anonymous, Some(access))?,
                ..self.clone()
            }));
        }
        if !mfa {
            return Err(Error::ResponseShape);
        }
        let field = |name: &str| {
            result[name]
                .as_str()
                .map(str::to_owned)
                .ok_or(Error::ResponseShape)
        };
        Ok(Login::MfaRequired(MfaChallenge {
            token: field("mfaToken")?,
            session_id: field("mfaSessionId")?,
            user_id: field("userId")?,
            user_email: field("userEmail")?,
        }))
    }

    pub async fn list_applets(&self, paging: Paging) -> Result<AppletPage, Error> {
        self.applets(Some(paging)).await
    }

    /// Look up an ID using CTK's unpaged list request. Not guaranteed exhaustive.
    pub async fn lookup_applet_by_id(&self, id: Uuid) -> Result<Applet, Error> {
        let result = self.applet_list_response(None).await?;
        let row = result["result"]
            .as_array()
            .ok_or(Error::ResponseShape)?
            .iter()
            .find(|row| {
                row["id"]
                    .as_str()
                    .and_then(|value| Uuid::parse_str(value).ok())
                    == Some(id)
            })
            .ok_or(Error::AppletNotFound)?;
        let applet = self.applet(row.clone())?;
        applet.encryption()?;
        Ok(applet)
    }

    async fn applets(&self, paging: Option<Paging>) -> Result<AppletPage, Error> {
        let result = self.applet_list_response(paging).await?;
        let applets = result["result"].as_array().ok_or(Error::ResponseShape)?;
        Ok(AppletPage {
            applets: applets
                .iter()
                .map(|row| self.applet(row.clone()))
                .collect::<Result<_, _>>()?,
            count: result["count"].as_i64(),
        })
    }

    async fn applet_list_response(&self, paging: Option<Paging>) -> Result<Value, Error> {
        decoded(
            self.reads
                .applet_list_applets_get(
                    OMIT,
                    paging.map(|p| p.page),
                    paging.map(|p| p.size),
                    OMIT,
                    OMIT,
                    OMIT,
                    None,
                )
                .await,
            1,
        )
    }

    pub async fn get_applet(&self, id: Uuid) -> Result<Applet, Error> {
        let mut result = decoded(
            self.reads
                .applet_retrieve_applets_applet_id_get(id.to_string())
                .await,
            1,
        )?;
        let metadata = result.get_mut("result").ok_or(Error::ResponseShape)?.take();
        let applet = self.applet(metadata)?;
        if applet.id != id {
            return Err(Error::ResponseShape);
        }
        Ok(applet)
    }

    fn applet(&self, metadata: Value) -> Result<Applet, Error> {
        let id = metadata["id"]
            .as_str()
            .and_then(|v| Uuid::parse_str(v).ok())
            .ok_or(Error::ResponseShape)?;
        let name = metadata["displayName"]
            .as_str()
            .ok_or(Error::ResponseShape)?
            .to_owned();
        Ok(Applet {
            origin: self.origin.clone(),
            id,
            name,
            metadata,
        })
    }

    /// Fetch one answer export page. The default size is 50; larger sizes
    /// are accepted, but can cause more gateway timeouts. Try smaller pages
    /// if timeouts persist. This method never advances the page automatically.
    pub async fn export_applet_data(
        &self,
        id: Uuid,
        from_date: Option<NaiveDate>,
        paging: Paging,
    ) -> Result<ExportPage, Error> {
        let id_text = id.to_string();
        let date = from_date.map(|v| v.format("%Y-%m-%d").to_string());
        let result = retry_export(
            || async {
                decoded(
                    self.reads
                        .applet_answers_export_answers_applet_applet_id_data_get(
                            &id_text,
                            None,
                            OMIT,
                            Some(paging.page),
                            Some(paging.size),
                            OMIT,
                            OMIT,
                            OMIT,
                            date.as_deref(),
                            OMIT,
                            None,
                            None,
                        )
                        .await,
                    1,
                )
            },
            |delay| tokio::time::sleep(delay),
            |low, high| rand::rng().random_range(low..=high),
        )
        .await?;
        export_page(&self.origin, id, result)
    }
}

fn origin(base_url: &str) -> Result<String, Error> {
    let url = parse_origin(base_url)?;
    if url.scheme() != "https" {
        return Err(Error::InvalidOrigin);
    }
    Ok(url.origin().ascii_serialization())
}

fn parse_origin(base_url: &str) -> Result<Url, Error> {
    let url = Url::parse(base_url).map_err(|_| Error::InvalidOrigin)?;
    if url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::InvalidOrigin);
    }
    Ok(url)
}

fn with_token(anonymous: &HttpClient, token: Option<&str>) -> Result<HttpClient, Error> {
    if let Some(token) = token {
        if token.is_empty() || !token.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err(Error::InvalidToken);
        }
        return Ok(anonymous.clone().with_api_key(token));
    }
    Ok(anonymous.clone())
}

fn export_page(origin: &str, applet_id: Uuid, mut response: Value) -> Result<ExportPage, Error> {
    let fields = response
        .get_mut("result")
        .and_then(Value::as_object_mut)
        .ok_or(Error::ResponseShape)?;
    let (Some(Value::Array(answers)), Some(Value::Array(activities))) =
        (fields.remove("answers"), fields.remove("activities"))
    else {
        return Err(Error::ResponseShape);
    };
    Ok(ExportPage {
        origin: origin.to_owned(),
        applet_id,
        answers,
        activities,
    })
}

fn operation_error<E: fmt::Debug>(error: ApiOpError<E>, attempts: u8) -> Error {
    match error {
        ApiOpError::Api(error) if error.status == 200 => Error::ResponseShape,
        ApiOpError::Api(error) => Error::HttpStatus {
            status: error.status,
            attempts,
        },
        ApiOpError::Transport(HttpError::ResponseTooLarge { limit }) => {
            Error::ResponseTooLarge { limit }
        }
        ApiOpError::Transport(_) => Error::Transport,
    }
}

fn model_value(model: impl serde::Serialize) -> Result<Value, Error> {
    serde_json::to_value(model).map_err(|_| Error::ResponseShape)
}

fn decoded<E: fmt::Debug>(
    response: Result<impl serde::Serialize, ApiOpError<E>>,
    attempts: u8,
) -> Result<Value, Error> {
    match response {
        Ok(model) => model_value(model),
        // Some servers omit optional-in-practice fields required in the spec.
        // Only recover a parsed 200 response, never an HTTP or body-read error.
        Err(ApiOpError::Api(error)) if error.status == 200 => {
            serde_json::from_slice(&error.raw_body).map_err(|_| Error::ResponseShape)
        }
        Err(error) => Err(operation_error(error, attempts)),
    }
}

async fn retry_export<T, F: Future<Output = Result<T, Error>>, S: Future<Output = ()>>(
    mut send: impl FnMut() -> F,
    mut sleep: impl FnMut(Duration) -> S,
    mut jitter: impl FnMut(u64, u64) -> u64,
) -> Result<T, Error> {
    for attempt in 1..=MAX_ATTEMPTS {
        match send().await {
            Err(Error::HttpStatus { status, .. }) if attempt < MAX_ATTEMPTS && status != 200 => {
                let low = 500 * (1 << (attempt - 1));
                sleep(Duration::from_millis(jitter(low, low * 2))).await;
            }
            Err(Error::HttpStatus { status, .. }) => {
                return Err(Error::HttpStatus {
                    status,
                    attempts: attempt,
                });
            }
            other => return other,
        }
    }
    unreachable!("final attempt always returns")
}

#[cfg(test)]
mod tests {
    use std::future::ready;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use serde_json::json;

    use super::*;

    fn status(status: u16) -> Error {
        Error::HttpStatus {
            status,
            attempts: 1,
        }
    }

    fn applet_response_server(body: Value) -> (Client, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let size = stream.read(&mut buffer).unwrap();
                assert_ne!(size, 0);
                request.extend_from_slice(&buffer[..size]);
            }
            let body = body.to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
            String::from_utf8(request).unwrap()
        });
        (
            Client::for_mock(&format!("http://{address}"), None, None).unwrap(),
            server,
        )
    }

    #[tokio::test]
    async fn lookup_ignores_unrelated_invalid_rows_and_sends_no_paging() {
        let id = Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa").unwrap();
        let body = json!({"result": [
            {"id": "not-a-uuid"},
            {"id": "00000000-0000-4000-8000-000000000001"},
            null,
            {"id": "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA", "displayName": "Target",
             "encryption": {"prime": "[1,1]", "accountId": "account"}},
            {"displayName": 7}
        ]});
        let (client, server) = applet_response_server(body);
        let applet = client.lookup_applet_by_id(id).await.unwrap();
        assert_eq!(applet.id(), id);
        assert_eq!(applet.name(), "Target");
        assert_eq!(
            server.join().unwrap().lines().next(),
            Some("GET /applets HTTP/1.1")
        );
    }

    #[tokio::test]
    async fn lookup_preserves_errors_for_missing_or_invalid_target() {
        let id = Uuid::nil();
        for (body, expected) in [
            (
                json!({"result": [{"id": "invalid"}]}),
                Error::AppletNotFound,
            ),
            (
                json!({"result": [{"id": id.to_string()}]}),
                Error::ResponseShape,
            ),
            (
                json!({"result": [{"id": id.to_string(), "displayName": "Target"}]}),
                Error::MissingEncryption,
            ),
            (json!({"result": {}}), Error::ResponseShape),
        ] {
            let (client, server) = applet_response_server(body);
            assert_eq!(client.lookup_applet_by_id(id).await.unwrap_err(), expected);
            server.join().unwrap();
        }
    }

    #[tokio::test]
    async fn lookup_does_not_weaken_list_applet_validation() {
        let (client, server) = applet_response_server(json!({"result": [
            {"id": "00000000-0000-4000-8000-000000000001"},
            {"id": Uuid::nil().to_string(), "displayName": "Target"}
        ]}));
        assert_eq!(
            client.list_applets(Paging::default()).await.unwrap_err(),
            Error::ResponseShape
        );
        assert_eq!(
            server.join().unwrap().lines().next(),
            Some("GET /applets?page=1&limit=50 HTTP/1.1")
        );
    }

    #[test]
    fn paging_defaults_to_50_and_does_not_clamp() {
        assert_eq!(Paging::default(), Paging { page: 1, size: 50 });
        assert_eq!(Paging::new(2, 79).unwrap(), Paging { page: 2, size: 79 });
        assert_eq!(Paging::new(0, 50), Err(Error::InvalidPaging));
        assert_eq!(Paging::new(1, -1), Err(Error::InvalidPaging));
    }

    #[tokio::test]
    async fn retries_distinct_non_200_statuses_and_then_returns_the_page() {
        let mut responses = [Err(status(201)), Err(status(302)), Ok("page")].into_iter();
        let mut delays = Vec::new();
        let result = retry_export(
            || ready(responses.next().unwrap()),
            |delay| {
                delays.push(delay);
                ready(())
            },
            |low, _| low,
        )
        .await;
        assert_eq!(result, Ok("page"));
        assert_eq!(delays, [Duration::from_millis(500), Duration::from_secs(1)]);
    }

    #[tokio::test]
    async fn does_not_retry_transport_or_shape_errors() {
        for error in [Error::Transport, Error::ResponseShape] {
            let mut response = Some(error);
            let result = retry_export::<(), _, _>(
                || ready(Err(response.take().unwrap())),
                |_| -> std::future::Ready<()> { panic!("unexpected delay") },
                |low, _| low,
            )
            .await;
            assert!(matches!(
                result,
                Err(Error::Transport | Error::ResponseShape)
            ));
        }
    }

    #[test]
    fn rejects_insecure_or_credentialed_origins() {
        for url in [
            "http://example.com",
            "https://alice:secret@example.com",
            "https://example.com/path",
        ] {
            assert_eq!(origin(url), Err(Error::InvalidOrigin));
        }
    }

    #[test]
    fn export_requires_both_arrays_and_accepts_an_empty_answer_page() {
        for input in [
            json!({}),
            json!({"result": null}),
            json!({"result": {"answers": []}}),
        ] {
            assert!(matches!(
                export_page("https://example.invalid", Uuid::nil(), input),
                Err(Error::ResponseShape)
            ));
        }
        let result = json!({"result": {"answers": [], "activities": [{"version": 7} ]}});
        let page = export_page("https://example.invalid", Uuid::nil(), result).unwrap();
        assert!(page.answers().is_empty());
        assert_eq!(page.activities(), [json!({"version": 7})]);
    }
}
