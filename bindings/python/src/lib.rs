use std::sync::Arc;

use curious_sdk as sdk;
use pyo3::{create_exception, exceptions::PyException, prelude::*, types::PyDict};
use tokio::sync::Semaphore;

create_exception!(_curious_sdk, CuriousError, PyException);
create_exception!(_curious_sdk, HttpStatusError, CuriousError);

fn error(error: sdk::Error) -> PyErr {
    if let sdk::Error::HttpStatus { status, attempts } = error {
        return Python::attach(|py| {
            let exception = HttpStatusError::new_err(error.to_string());
            let value = exception.value(py);
            value.setattr("status", status).expect("exception supports attributes");
            value.setattr("attempts", attempts).expect("exception supports attributes");
            exception
        });
    }
    CuriousError::new_err(error.to_string())
}

fn applet_id(id: &str) -> PyResult<sdk::Uuid> {
    sdk::Uuid::parse_str(id).map_err(|_| CuriousError::new_err("invalid applet UUID"))
}

fn decoded_json(py: Python<'_>, json: String) -> PyResult<Py<PyAny>> {
    let kwargs = PyDict::new(py);
    kwargs.set_item("parse_float", py.import("decimal")?.getattr("Decimal")?)?;
    Ok(py.import("json")?.getattr("loads")?.call((json,), Some(&kwargs))?.unbind())
}

fn records(py: Python<'_>, values: &[serde_json::Value]) -> PyResult<Py<PyAny>> {
    let json = serde_json::to_string(values).map_err(|_| error(sdk::Error::ResponseShape))?;
    decoded_json(py, json)
}

/// Immutable Curious client configuration and session.
#[pyclass(frozen, module = "curious_sdk")]
#[derive(Clone)]
struct Client(sdk::Client);

#[pymethods]
impl Client {
    #[new]
    #[pyo3(signature = (base_url, bearer_token=None))]
    fn new(base_url: &str, bearer_token: Option<&str>) -> PyResult<Self> {
        sdk::Client::new(base_url, bearer_token).map(Self).map_err(error)
    }

    /// Connect to a loopback HTTP Prism mock. Use synthetic credentials only.
    #[staticmethod]
    #[pyo3(signature = (base_url, bearer_token=None, *, prefer=None))]
    fn for_mock(base_url: &str, bearer_token: Option<&str>, prefer: Option<&str>) -> PyResult<Self> {
        sdk::Client::for_mock(base_url, bearer_token, prefer).map(Self).map_err(error)
    }

    /// Return a new authenticated Client or MfaChallenge; leave this client unchanged.
    fn login<'py>(&self, py: Python<'py>, email: String, password: String) -> PyResult<Bound<'py, PyAny>> {
        let client = self.0.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let login = client.login(&email, &password).await.map_err(error)?;
            Python::attach(|py| match login {
                sdk::Login::Authenticated(client) => Py::new(py, Self(client)).map(Py::into_any),
                sdk::Login::MfaRequired(challenge) => Py::new(py, MfaChallenge(challenge)).map(Py::into_any),
            })
        })
    }

    /// List one applet page. Defaults to 50. Larger pages can cause gateway
    /// timeouts; use smaller pages if timeouts persist.
    #[pyo3(signature = (page=1, page_size=sdk::DEFAULT_PAGE_SIZE))]
    fn list_applets<'py>(&self, py: Python<'py>, page: i64, page_size: i64) -> PyResult<Bound<'py, PyAny>> {
        let paging = sdk::Paging::new(page, page_size).map_err(error)?;
        let client = self.0.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            client.list_applets(paging).await.map(AppletPage).map_err(error)
        })
    }

    /// Fetch applet detail by UUID.
    fn get_applet<'py>(&self, py: Python<'py>, id: &str) -> PyResult<Bound<'py, PyAny>> {
        let id = applet_id(id)?;
        let client = self.0.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            client.get_applet(id).await.map(Applet).map_err(error)
        })
    }

    /// Find an applet in CTK's unpaged list. The server list may not be exhaustive.
    fn lookup_applet_by_id<'py>(&self, py: Python<'py>, id: &str) -> PyResult<Bound<'py, PyAny>> {
        let id = applet_id(id)?;
        let client = self.0.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            client.lookup_applet_by_id(id).await.map(Applet).map_err(error)
        })
    }

    /// Fetch one export page. The default page size is 50. Larger pages are
    /// accepted but can increase gateway timeouts; try smaller pages if they persist.
    /// Supply from_date as YYYY-MM-DD, or omit it to send no date filter.
    #[pyo3(signature = (id, *, from_date=None, page=1, page_size=sdk::DEFAULT_PAGE_SIZE))]
    fn export_applet_data<'py>(&self, py: Python<'py>, id: &str, from_date: Option<&str>, page: i64, page_size: i64) -> PyResult<Bound<'py, PyAny>> {
        let id = applet_id(id)?;
        let paging = sdk::Paging::new(page, page_size).map_err(error)?;
        let from_date = from_date.map(|value| {
            let date = sdk::NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .map_err(|_| CuriousError::new_err("expected from_date as YYYY-MM-DD"))?;
            if date.format("%Y-%m-%d").to_string() != value || value.len() != 10 {
                return Err(CuriousError::new_err("expected from_date as YYYY-MM-DD"));
            }
            Ok(date)
        }).transpose()?;
        let client = self.0.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            client.export_applet_data(id, from_date, paging).await.map(ExportPage).map_err(error)
        })
    }
}

/// A challenge from Curious. This SDK does not complete MFA.
#[pyclass(frozen, module = "curious_sdk")]
struct MfaChallenge(sdk::MfaChallenge);

#[pymethods]
impl MfaChallenge {
    #[getter]
    fn token(&self) -> &str { self.0.token() }
    #[getter]
    fn session_id(&self) -> &str { self.0.session_id() }
    #[getter]
    fn user_id(&self) -> &str { self.0.user_id() }
    #[getter]
    fn user_email(&self) -> &str { self.0.user_email() }
}

/// An applet and its decoded metadata.
#[pyclass(frozen, module = "curious_sdk")]
struct Applet(sdk::Applet);

#[pymethods]
impl Applet {
    #[getter]
    fn id(&self) -> String { self.0.id().to_string() }
    #[getter]
    fn name(&self) -> &str { self.0.name() }
    #[getter]
    fn metadata(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        decoded_json(py, self.0.metadata().to_string())
    }
}

/// One applet list page.
#[pyclass(frozen, module = "curious_sdk")]
struct AppletPage(sdk::AppletPage);

#[pymethods]
impl AppletPage {
    #[getter]
    fn applets(&self) -> Vec<Applet> { self.0.applets().iter().cloned().map(Applet).collect() }
    /// Optional count provided by Curious; not necessarily a total.
    #[getter]
    fn count(&self) -> Option<i64> { self.0.count() }
}

/// One decoded export page, without original HTTP response data.
#[pyclass(frozen, module = "curious_sdk")]
struct ExportPage(sdk::ExportPage);

#[pymethods]
impl ExportPage {
    #[getter]
    fn answers(&self, py: Python<'_>) -> PyResult<Py<PyAny>> { records(py, self.0.answers()) }
    #[getter]
    fn activities(&self, py: Python<'_>) -> PyResult<Py<PyAny>> { records(py, self.0.activities()) }
}

/// Export page and matching applet encryption metadata.
#[pyclass(frozen, module = "curious_sdk")]
struct EncryptedData(Arc<sdk::EncryptedData>);

/// A new decrypted page; inspect outcomes to identify retained ciphertext.
#[pyclass(frozen, module = "curious_sdk")]
struct DecryptedData(sdk::DecryptedData);

#[pymethods]
impl DecryptedData {
    #[getter]
    fn answers(&self, py: Python<'_>) -> PyResult<Py<PyAny>> { records(py, self.0.answers()) }
    #[getter]
    fn activities(&self, py: Python<'_>) -> PyResult<Py<PyAny>> { records(py, self.0.activities()) }
    /// Pairs of (answer, events) outcomes in record order.
    #[getter]
    fn outcomes(&self) -> Vec<(&'static str, &'static str)> {
        self.0.outcomes().iter().map(|v| (outcome(v.answer), outcome(v.events))).collect()
    }
}

fn outcome(value: sdk::FieldOutcome) -> &'static str {
    match value {
        sdk::FieldOutcome::Unchanged => "unchanged",
        sdk::FieldOutcome::MissingOrInvalidKey => "missing_or_invalid_key",
        sdk::FieldOutcome::Failed => "failed",
        sdk::FieldOutcome::Decrypted => "decrypted",
    }
}

/// Combine an export page with matching applet metadata without modifying either.
#[pyfunction]
fn encrypted_data(page: &ExportPage, applet: &Applet) -> PyResult<EncryptedData> {
    sdk::encrypted_data(page.0.clone(), &applet.0)
        .map(|value| EncryptedData(Arc::new(value))).map_err(error)
}

static DECRYPT_SLOTS: Semaphore = Semaphore::const_new(2);

/// Return a new decrypted value without performing I/O or blocking the event loop.
#[pyfunction]
fn decrypt<'py>(py: Python<'py>, password: String, data: &EncryptedData) -> PyResult<Bound<'py, PyAny>> {
    let data = Arc::clone(&data.0);
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let permit = DECRYPT_SLOTS.acquire().await.expect("static semaphore is never closed");
        tokio::task::spawn_blocking(move || {
            let _permit = permit; // Keep capacity until the job, not its awaiter, finishes.
            DecryptedData(sdk::decrypt(&password, &data))
        }).await.map_err(|_| CuriousError::new_err("decryption worker failed"))
    })
}

#[pymodule]
fn _curious_sdk(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add("CuriousError", module.py().get_type::<CuriousError>())?;
    module.add("HttpStatusError", module.py().get_type::<HttpStatusError>())?;
    module.add_class::<Client>()?;
    module.add_class::<MfaChallenge>()?;
    module.add_class::<Applet>()?;
    module.add_class::<AppletPage>()?;
    module.add_class::<ExportPage>()?;
    module.add_class::<EncryptedData>()?;
    module.add_class::<DecryptedData>()?;
    module.add_function(wrap_pyfunction!(encrypted_data, module)?)?;
    module.add_function(wrap_pyfunction!(decrypt, module)?)?;
    Ok(())
}
