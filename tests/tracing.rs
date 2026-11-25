use std::{
    io::{BufRead, BufReader},
    net::TcpListener,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use curious_sdk::{Client, Error, Uuid};
use tracing::{
    Event, Metadata, Subscriber,
    field::{Field, Visit},
    span::{Attributes, Id, Record},
};

#[derive(Default)]
struct MiddlewareCapture {
    fields: Arc<Mutex<Vec<String>>>,
    next_id: AtomicU64,
}

struct FieldVisitor<'a>(&'a Mutex<Vec<String>>);

impl Visit for FieldVisitor<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0
            .lock()
            .unwrap()
            .push(format!("{}={value:?}", field.name()));
    }
}

impl Subscriber for MiddlewareCapture {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.target().starts_with("reqwest_tracing")
    }

    fn new_span(&self, attrs: &Attributes<'_>) -> Id {
        attrs.record(&mut FieldVisitor(&self.fields));
        Id::from_u64(self.next_id.fetch_add(1, Ordering::Relaxed) + 1)
    }

    fn record(&self, _span: &Id, values: &Record<'_>) {
        values.record(&mut FieldVisitor(&self.fields));
    }

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        event.record(&mut FieldVisitor(&self.fields));
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

#[tokio::test]
async fn failed_sdk_request_does_not_put_its_url_in_middleware_telemetry() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let id = Uuid::parse_str("00000000-0000-4000-8000-000000000001").unwrap();
    let url = format!("{origin}/applets/{id}");
    let server = thread::spawn(move || {
        for _ in 0..2 {
            let (socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = BufReader::new(socket);
            let mut line = String::new();
            loop {
                line.clear();
                assert_ne!(request.read_line(&mut line).unwrap(), 0);
                if line == "\r\n" {
                    break;
                }
            }
            // Close without a response, forcing a transport error after the request was sent.
        }
    });

    let control = MiddlewareCapture::default();
    let control_fields = Arc::clone(&control.fields);
    {
        let _guard = tracing::subscriber::set_default(control);
        let transport = reqwest::Client::builder()
            .no_proxy()
            .retry(reqwest::retry::never())
            .build()
            .unwrap();
        let client = reqwest_middleware::ClientBuilder::new(transport)
            .with(reqwest_tracing::TracingMiddleware::default())
            .build();
        assert!(client.get(&url).send().await.is_err());
    }
    assert!(
        control_fields
            .lock()
            .unwrap()
            .iter()
            .any(|field| field.contains(&url)),
        "positive control must capture the failed request URL"
    );

    let capture = MiddlewareCapture::default();
    let fields = Arc::clone(&capture.fields);
    {
        let _guard = tracing::subscriber::set_default(capture);
        let client = Client::for_mock(&origin, None, None).unwrap();
        assert_eq!(client.get_applet(id).await.unwrap_err(), Error::Transport);
    }
    server.join().unwrap();
    assert!(
        fields
            .lock()
            .unwrap()
            .iter()
            .all(|field| !field.contains(&url)),
        "SDK middleware telemetry must not contain the failed request URL"
    );
}
