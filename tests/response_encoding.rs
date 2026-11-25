use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    thread,
    time::Duration,
};

use curious_sdk::{Client, Error, Paging, Uuid};

async fn export_with_activity_field(field: &str, value: &[u8]) -> Result<curious_sdk::ExportPage, Error> {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let mut body = br#"{"result":{"answers":[],"activities":[{"id":"00000000-0000-4000-8000-000000000005","createdAt":"2026-01-01T00:00:00Z","idVersion":"fixture-1","name":"fixture",""#.to_vec();
    body.extend_from_slice(field.as_bytes());
    body.extend_from_slice(b"\":\"");
    body.extend_from_slice(value);
    body.extend_from_slice(br#""}]}}"#);

    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut request = BufReader::new(&mut socket);
        let mut line = String::new();
        loop {
            line.clear();
            assert_ne!(request.read_line(&mut line).unwrap(), 0);
            if line == "\r\n" {
                break;
            }
        }
        write!(
            socket,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        socket.write_all(&body).unwrap();
    });

    let result = Client::for_mock(&origin, None, None)
        .unwrap()
        .export_applet_data(Uuid::nil(), None, Paging::default())
        .await;
    server.join().unwrap();
    result
}

#[tokio::test]
async fn rejects_invalid_utf8_in_export_json_strings() {
    let result = export_with_activity_field("description", b"activity \xff").await;
    assert_eq!(result.unwrap_err(), Error::ResponseShape);
}

#[tokio::test]
async fn rejects_invalid_utf8_even_in_unknown_fields() {
    let result = export_with_activity_field("unknownField", b"activity \xff").await;
    assert_eq!(result.unwrap_err(), Error::ResponseShape);
}

#[tokio::test]
async fn preserves_a_valid_unicode_replacement_character() {
    let name = "activity \u{fffd}";
    let page = export_with_activity_field("description", name.as_bytes()).await.unwrap();
    assert_eq!(page.activities()[0]["description"], name);
}
