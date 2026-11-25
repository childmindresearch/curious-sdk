use curious_sdk::{Client, Error, FieldOutcome, Login, Paging, Uuid, decrypt, encrypted_data};

const ID: &str = "00000000-0000-4000-8000-000000000001";

fn mock_url() -> String {
    std::env::var("CURIOUS_MOCK_URL").unwrap_or_else(|_| "http://127.0.0.1:4010".into())
}

#[tokio::test]
#[ignore = "requires the fixed Prism fixtures from just mock-sdk"]
async fn logs_in_and_exports_and_decrypts_one_page() {
    let client = Client::for_mock(&mock_url(), None, Some("example=sdk_fixture")).unwrap();
    let Login::Authenticated(client) = client.login("fixture@example.invalid", "fixture-login").await.unwrap() else {
        panic!("the Prism login fixture must return a token");
    };
    let id = Uuid::parse_str(ID).unwrap();
    let list = client.list_applets(Paging::default()).await.unwrap();
    assert!(
        list.applets().iter().any(|applet| applet.id() == id),
        "Prism did not return the sdk_fixture applet; restart the server with `just mock-sdk`"
    );
    assert_eq!(client.lookup_applet_by_id(id).await.unwrap().id(), id);

    let applet = client.get_applet(id).await.unwrap();
    assert_eq!(applet.name(), "Prism SDK fixture");
    let page = client.export_applet_data(id, None, Paging::default()).await.unwrap();
    assert_eq!(page.answers().len(), 1);
    assert_eq!(page.activities()[0]["name"], "Fixture activity");
    let data = encrypted_data(page.clone(), &applet).unwrap();
    let decoded = decrypt("fixture-password", &data);
    assert_eq!(decoded.answers()[0]["answer"], serde_json::json!([null, 7]));
    assert_eq!(decoded.outcomes()[0].answer, FieldOutcome::Decrypted);
    assert!(page.answers()[0]["answer"].is_string());
}

#[tokio::test]
#[ignore = "requires the Prism server from just mock or just mock-sdk"]
async fn retries_a_non_gateway_http_error_until_the_attempt_budget_is_exhausted() {
    let client = Client::for_mock(&mock_url(), Some("fixture-token"), Some("code=401")).unwrap();
    let result = client.export_applet_data(Uuid::parse_str(ID).unwrap(), None, Paging::new(2, 51).unwrap()).await;
    assert_eq!(result.unwrap_err(), Error::HttpStatus { status: 401, attempts: 3 });
}
