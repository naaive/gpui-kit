//! The driver against a stand-in for ClickHouse's HTTP interface.
//!
//! The stand-in answers the way ClickHouse does — the same headers, the same
//! `TabSeparatedWithNamesAndTypes` bodies, the same exception text — so
//! these tests prove what the driver sends and how it reads the answers on
//! any machine. `tests/clickhouse.rs` checks the same against a real server.

use std::{
    io::{Read as _, Write as _},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use datakit_catalog::RelationType;
use datakit_driver::{
    Connection, ConnectionProfile, DatabaseError, Driver as _, SslMode, StatementOutcome, Value,
};
use datakit_driver_clickhouse::ClickHouseDriver;
use datakit_runtime::IoRuntime;
use futures::StreamExt as _;

#[derive(Clone, Debug)]
struct Request {
    query: Vec<(String, String)>,
    headers: Vec<(String, String)>,
    body: String,
}

impl Request {
    fn param(&self, name: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

type Handler = dyn Fn(&Request, &mut TcpStream) + Send + Sync;

struct Server {
    port: u16,
    requests: Arc<Mutex<Vec<Request>>>,
}

impl Server {
    /// A server that answers the driver's connection probe itself and every
    /// other request with `handler`, one connection per request.
    fn start(handler: impl Fn(&Request, &mut TcpStream) + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let handler: Arc<Handler> = Arc::new(handler);
        thread::spawn({
            let requests = requests.clone();
            move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { continue };
                    let handler = handler.clone();
                    let requests = requests.clone();
                    thread::spawn(move || {
                        let Some(request) = read_request(&mut stream) else {
                            return;
                        };
                        requests.lock().unwrap().push(request.clone());
                        if request
                            .body
                            .starts_with("SELECT version(), currentDatabase()")
                        {
                            respond(
                                &mut stream,
                                200,
                                &[],
                                "version()\tcurrentDatabase()\nString\tString\n24.8.1.1\tshop\n",
                            );
                        } else {
                            handler(&request, &mut stream);
                        }
                    });
                }
            }
        });
        Self { port, requests }
    }

    fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }

    /// The first request whose statement starts with `prefix`.
    fn request(&self, prefix: &str) -> Option<Request> {
        self.requests()
            .into_iter()
            .find(|request| request.body.starts_with(prefix))
    }

    fn connect(&self, runtime: &IoRuntime) -> Arc<dyn Connection> {
        let profile = ConnectionProfile::new(ClickHouseDriver::ID, self.port)
            .with_host("127.0.0.1")
            .with_user("alice")
            .with_database("shop")
            .with_ssl_mode(SslMode::Prefer);
        runtime
            .block_on(ClickHouseDriver.connect(&profile, Some("secret".into())))
            .expect("connect")
    }
}

fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut data = Vec::new();
    let mut buffer = [0; 4096];
    let head_end = loop {
        let read = stream.read(&mut buffer).ok()?;
        if read == 0 {
            return None;
        }
        data.extend_from_slice(&buffer[..read]);
        if let Some(end) = data.windows(4).position(|window| window == b"\r\n\r\n") {
            break end;
        }
    };
    let head = String::from_utf8_lossy(&data[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let target = lines.next()?.split(' ').nth(1)?.to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(": "))
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    let length: usize = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.parse().ok())
        .unwrap_or(0);
    let mut body = data[head_end + 4..].to_vec();
    while body.len() < length {
        let read = stream.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&buffer[..read]);
    }
    let query = target
        .split_once('?')
        .map(|(_, query)| query)
        .unwrap_or_default()
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(key, value)| (decode(key), decode(value)))
        .collect();
    Some(Request {
        query,
        headers,
        body: String::from_utf8(body).ok()?,
    })
}

fn decode(text: &str) -> String {
    let mut bytes = Vec::new();
    let mut iter = text.bytes();
    while let Some(byte) = iter.next() {
        match byte {
            b'+' => bytes.push(b' '),
            b'%' => {
                let hex: Vec<u8> = iter.by_ref().take(2).collect();
                let hex = std::str::from_utf8(&hex).unwrap();
                bytes.push(u8::from_str_radix(hex, 16).unwrap());
            }
            byte => bytes.push(byte),
        }
    }
    String::from_utf8(bytes).unwrap()
}

fn respond(stream: &mut TcpStream, status: u16, headers: &[(&str, &str)], body: &str) {
    let mut response = format!("HTTP/1.1 {status} Status\r\nConnection: close\r\n");
    for (name, value) in headers {
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    response.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
    let _ = stream.write_all(response.as_bytes());
}

/// Start a response whose body runs until the connection closes, as a
/// streamed ClickHouse result does.
fn start_streaming(stream: &mut TcpStream, first: &str) {
    let _ =
        stream.write_all(format!("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{first}").as_bytes());
    let _ = stream.flush();
}

fn wait_for(what: &str, condition: impl Fn() -> bool) {
    let started = Instant::now();
    while !condition() {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "timed out waiting for {what}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn statements_run_in_one_session_and_stream_typed_rows() {
    let server = Server::start(|_, stream| {
        respond(
            stream,
            200,
            &[("X-ClickHouse-Format", "TabSeparatedWithNamesAndTypes")],
            "n\tnote\tok\tbig\nUInt64\tNullable(String)\tBool\tUInt64\n\
             1\ttab\\there\ttrue\t18446744073709551615\n2\t\\N\tfalse\t0\n",
        );
    });
    let runtime = IoRuntime::new().unwrap();
    let connection = server.connect(&runtime);
    assert_eq!(&*connection.server_version(), "ClickHouse 24.8.1.1");

    let StatementOutcome::Rows(stream) = runtime
        .block_on(connection.execute("SELECT n, note, ok, big FROM t".into()))
        .unwrap()
    else {
        panic!("expected rows");
    };
    let types: Vec<String> = stream
        .columns()
        .iter()
        .map(|column| column.type_name().to_string())
        .collect();
    assert_eq!(types, ["UInt64", "Nullable(String)", "Bool", "UInt64"]);
    let rows: Vec<Vec<Value>> =
        runtime.block_on(stream.map(|row| row.unwrap().into_vec()).collect());
    assert_eq!(
        rows,
        [
            vec![
                Value::Int(1),
                Value::Text("tab\there".into()),
                Value::Bool(true),
                Value::Text("18446744073709551615".into()),
            ],
            vec![
                Value::Int(2),
                Value::Null,
                Value::Bool(false),
                Value::Int(0)
            ],
        ]
    );

    let probe = server.request("SELECT version()").unwrap();
    let select = server.request("SELECT n").unwrap();
    assert_eq!(select.header("X-ClickHouse-User"), Some("alice"));
    assert_eq!(select.header("X-ClickHouse-Key"), Some("secret"));
    assert_eq!(select.param("database"), Some("shop"));
    assert_eq!(
        select.param("default_format"),
        Some("TabSeparatedWithNamesAndTypes")
    );
    assert!(select.param("session_id").is_some());
    assert_eq!(select.param("session_id"), probe.param("session_id"));
    assert_ne!(select.param("query_id"), probe.param("query_id"));
}

#[test]
fn commands_report_the_rows_an_insert_wrote() {
    let server = Server::start(|request, stream| {
        let summary = if request.body.starts_with("INSERT") {
            r#"{"read_rows":"3","written_rows":"3"}"#
        } else {
            r#"{"read_rows":"0","written_rows":"0"}"#
        };
        respond(stream, 200, &[("X-ClickHouse-Summary", summary)], "");
    });
    let runtime = IoRuntime::new().unwrap();
    let connection = server.connect(&runtime);

    let StatementOutcome::Command(inserted) = runtime
        .block_on(connection.execute("INSERT INTO t SELECT number FROM numbers(3)".into()))
        .unwrap()
    else {
        panic!("expected a command");
    };
    assert_eq!((inserted.tag(), inserted.rows()), ("INSERT", Some(3)));
    assert_eq!(
        server.request("INSERT").unwrap().param("wait_end_of_query"),
        Some("1")
    );

    let StatementOutcome::Command(created) = runtime
        .block_on(connection.execute("CREATE TABLE t (a UInt8) ENGINE = Memory".into()))
        .unwrap()
    else {
        panic!("expected a command");
    };
    assert_eq!((created.tag(), created.rows()), ("CREATE TABLE", None));
    assert!(
        server
            .request("CREATE")
            .unwrap()
            .param("wait_end_of_query")
            .is_none()
    );

    // A comment alone is not sent.
    let StatementOutcome::Command(nothing) = runtime
        .block_on(connection.execute("-- nothing".into()))
        .unwrap()
    else {
        panic!("expected a command");
    };
    assert_eq!(nothing.tag(), "");
    assert!(server.request("--").is_none());
}

#[test]
fn errors_carry_the_code_and_the_position() {
    let server = Server::start(|_, stream| {
        respond(
            stream,
            400,
            &[("X-ClickHouse-Exception-Code", "62")],
            "Code: 62. DB::Exception: Syntax error: failed at position 10 ('FRM') (line 1, col 10): \
             FRM t. Expected one of: FROM, WHERE. (SYNTAX_ERROR) (version 24.8.1.1 (official build))\n",
        );
    });
    let runtime = IoRuntime::new().unwrap();
    let connection = server.connect(&runtime);
    let sql = "SELECT * FRM t";
    let Err(error) = runtime.block_on(connection.execute(sql.into())) else {
        panic!("the statement must fail");
    };
    let error = error
        .downcast_ref::<DatabaseError>()
        .expect("a database error");
    assert_eq!(error.code(), Some("62"));
    assert_eq!(&sql[error.position().unwrap()..], "FRM t");
    assert!(error.detail().unwrap().starts_with("Expected one of"));
}

#[test]
fn an_exception_after_the_first_rows_ends_the_stream_with_it() {
    let server = Server::start(|_, stream| {
        respond(
            stream,
            200,
            &[],
            "n\nUInt64\n1\n2\nCode: 241. DB::Exception: Memory limit (total) exceeded. \
             (MEMORY_LIMIT_EXCEEDED) (version 24.8.1.1)\n",
        );
    });
    let runtime = IoRuntime::new().unwrap();
    let connection = server.connect(&runtime);
    let StatementOutcome::Rows(stream) = runtime
        .block_on(connection.execute("SELECT number FROM numbers(1e10)".into()))
        .unwrap()
    else {
        panic!("expected rows");
    };
    let items: Vec<_> = runtime.block_on(stream.collect());
    assert_eq!(items.len(), 3);
    assert_eq!(items[1].as_ref().unwrap()[0], Value::Int(2));
    let error = items[2].as_ref().unwrap_err();
    let error = error.downcast_ref::<DatabaseError>().unwrap();
    assert_eq!(error.code(), Some("241"));
    assert_eq!(error.message(), "Memory limit (total) exceeded.");
    // The statement ended on its own: nothing is killed.
    thread::sleep(Duration::from_millis(100));
    assert!(server.request("KILL").is_none());
}

#[test]
fn a_dropped_result_is_killed_before_the_session_runs_again() {
    let killed = Arc::new(AtomicBool::new(false));
    let server = Server::start({
        let killed = killed.clone();
        move |request, stream| {
            if request.body.starts_with("KILL QUERY") {
                killed.store(true, Ordering::SeqCst);
                respond(stream, 200, &[], "");
            } else if request.body.starts_with("SELECT number") {
                start_streaming(stream, "number\nUInt64\n0\n1\n");
                // Keep the statement running until it is killed.
                let started = Instant::now();
                while !killed.load(Ordering::SeqCst) && started.elapsed() < Duration::from_secs(5) {
                    thread::sleep(Duration::from_millis(10));
                }
            } else {
                respond(stream, 200, &[], "42\nUInt8\n42\n");
            }
        }
    });
    let runtime = IoRuntime::new().unwrap();
    let connection = server.connect(&runtime);
    let StatementOutcome::Rows(mut stream) = runtime
        .block_on(connection.execute("SELECT number FROM system.numbers".into()))
        .unwrap()
    else {
        panic!("expected rows");
    };
    assert!(runtime.block_on(stream.next()).is_some());
    drop(stream);

    let started = Instant::now();
    let StatementOutcome::Rows(stream) = runtime
        .block_on(connection.execute("SELECT 42".into()))
        .unwrap()
    else {
        panic!("expected rows");
    };
    assert!(started.elapsed() < Duration::from_secs(3));
    let rows: Vec<_> = runtime.block_on(stream.collect());
    assert_eq!(rows[0].as_ref().unwrap()[0], Value::Int(42));

    let requests = server.requests();
    let position = |prefix: &str| {
        requests
            .iter()
            .position(|request| request.body.starts_with(prefix))
            .unwrap()
    };
    let select = &requests[position("SELECT number")];
    let kill = &requests[position("KILL QUERY")];
    assert_eq!(
        kill.body,
        format!(
            "KILL QUERY WHERE query_id = '{}' SYNC",
            select.param("query_id").unwrap()
        )
    );
    // Outside the session, which the abandoned statement still holds.
    assert!(kill.param("session_id").is_none());
    assert!(position("KILL QUERY") < position("SELECT 42"));
}

#[test]
fn cancelling_kills_the_running_statement() {
    let killed = Arc::new(AtomicBool::new(false));
    let server = Server::start({
        let killed = killed.clone();
        move |request, stream| {
            if request.body.starts_with("KILL QUERY") {
                killed.store(true, Ordering::SeqCst);
                respond(stream, 200, &[], "");
                return;
            }
            let started = Instant::now();
            while !killed.load(Ordering::SeqCst) && started.elapsed() < Duration::from_secs(5) {
                thread::sleep(Duration::from_millis(10));
            }
            respond(
                stream,
                500,
                &[("X-ClickHouse-Exception-Code", "394")],
                "Code: 394. DB::Exception: Query was cancelled. (QUERY_WAS_CANCELLED)\n",
            );
        }
    });
    let runtime = IoRuntime::new().unwrap();
    let connection = server.connect(&runtime);
    // Idle, cancelling sends nothing.
    runtime.block_on(connection.cancel()).unwrap();
    assert!(server.request("KILL").is_none());

    let running = runtime.spawn({
        let connection = connection.clone();
        async move { connection.execute("SELECT sleep(3)".into()).await }
    });
    wait_for("the statement", || server.request("SELECT sleep").is_some());
    runtime.block_on(connection.cancel()).unwrap();
    let Err(error) = futures::executor::block_on(running) else {
        panic!("the statement was cancelled");
    };
    let error = error.downcast_ref::<DatabaseError>().unwrap();
    assert_eq!(error.code(), Some("394"));

    let select = server.request("SELECT sleep").unwrap();
    let kill = server.request("KILL QUERY").unwrap();
    assert_eq!(
        kill.body,
        format!(
            "KILL QUERY WHERE query_id = '{}' ASYNC",
            select.param("query_id").unwrap()
        )
    );
}

#[test]
fn a_statement_with_its_own_format_returns_that_text() {
    let server = Server::start(|_, stream| {
        respond(
            stream,
            200,
            &[("X-ClickHouse-Format", "JSONEachRow")],
            "{\"n\":1}\n{\"n\":2}\n",
        );
    });
    let runtime = IoRuntime::new().unwrap();
    let connection = server.connect(&runtime);
    let StatementOutcome::Rows(stream) = runtime
        .block_on(
            connection.execute("SELECT number AS n FROM numbers(2) FORMAT JSONEachRow".into()),
        )
        .unwrap()
    else {
        panic!("expected rows");
    };
    assert_eq!(&*stream.columns()[0].name(), "JSONEachRow");
    let rows: Vec<Vec<Value>> =
        runtime.block_on(stream.map(|row| row.unwrap().into_vec()).collect());
    assert_eq!(
        rows,
        [
            vec![Value::Text("{\"n\":1}".into())],
            vec![Value::Text("{\"n\":2}".into())],
        ]
    );
}

#[test]
fn introspection_reads_the_system_tables() {
    let server = Server::start(|request, stream| {
        let body = &request.body;
        let answer = if body.contains("FROM system.databases WHERE") {
            "name\tengine\tcomment\nString\tString\tString\nshop\tAtomic\tThe shop\n"
        } else if body.contains("FROM system.databases") {
            "name\tengine\nString\tString\nsystem\tAtomic\nshop\tAtomic\ndefault\tAtomic\n"
        } else if body.contains("FROM system.tables") {
            "name\tengine\tengine_full\tcreate_table_query\tcomment\ttotal_rows\tprimary_key\n\
             String\tString\tString\tString\tString\tNullable(UInt64)\tString\n\
             orders\tMergeTree\tMergeTree ORDER BY (id, day) SETTINGS index_granularity = 8192\t\
             CREATE TABLE shop.orders (`id` UInt64, `day` Date, `note` Nullable(String), \
             CONSTRAINT positive CHECK id > 0) ENGINE = MergeTree ORDER BY (id, day)\tEvery order\t42\tid, day\n\
             recent\tView\t\tCREATE VIEW shop.recent (`id` UInt64) AS SELECT id FROM shop.orders\t\t\\N\t\n"
        } else if body.contains("FROM system.columns") {
            "table\tname\ttype\tdefault_kind\tdefault_expression\tcomment\tis_in_primary_key\n\
             String\tString\tString\tString\tString\tString\tUInt8\n\
             orders\tid\tUInt64\t\t\t\t1\n\
             orders\tday\tDate\tMATERIALIZED\ttoday()\t\t1\n\
             orders\tnote\tNullable(String)\tDEFAULT\t'none'\tFree text\t0\n\
             recent\tid\tUInt64\t\t\t\t0\n"
        } else if body.contains("FROM system.data_skipping_indices") {
            "table\tname\ttype\ttype_full\texpr\tgranularity\n\
             String\tString\tString\tString\tString\tUInt64\n\
             orders\tnote_bloom\tbloom_filter\tbloom_filter(0.01)\tnote\t4\n"
        } else if body.contains("FROM system.functions") {
            "name\tcreate_query\nString\tString\n\
             linear\tCREATE FUNCTION linear AS (x, k, b) -> ((k * x) + b)\n"
        } else if body.contains("currentDatabase()") {
            "currentDatabase()\nString\nshop\n"
        } else {
            ""
        };
        respond(stream, 200, &[], answer);
    });
    let runtime = IoRuntime::new().unwrap();
    let connection = server.connect(&runtime);

    let schemas = runtime.block_on(connection.introspect_schemas()).unwrap();
    let names: Vec<(String, bool)> = schemas
        .iter()
        .map(|schema| (schema.name().to_string(), schema.is_system()))
        .collect();
    assert_eq!(
        names,
        [
            ("default".into(), false),
            ("shop".into(), false),
            ("system".into(), true)
        ]
    );

    let shop = runtime
        .block_on(connection.introspect_schema("shop".into()))
        .unwrap();
    assert_eq!(shop.comment(), Some("The shop"));
    let tables = server
        .request("\n    SELECT *\n    FROM system.tables")
        .unwrap();
    assert_eq!(tables.param("param_schema"), Some("shop"));
    assert!(tables.param("session_id").is_none());

    let orders = shop.relation("orders").unwrap();
    assert_eq!(orders.relation_type(), RelationType::Table);
    assert_eq!(orders.comment(), Some("Every order"));
    assert_eq!(orders.estimated_rows(), Some(42));
    assert_eq!(
        orders.definition(),
        Some("MergeTree ORDER BY (id, day) SETTINGS index_granularity = 8192")
    );
    let key: Vec<String> = orders
        .primary_key()
        .iter()
        .map(|column| column.name().to_string())
        .collect();
    assert_eq!(key, ["id", "day"]);
    let day = orders.column("day").unwrap();
    assert!(day.is_generated());
    assert_eq!(day.default(), Some("today()"));
    let note = orders.column("note").unwrap();
    assert!(note.is_nullable() && !note.is_generated());
    assert_eq!(note.comment(), Some("Free text"));
    let indexes: Vec<(String, bool, Option<String>)> = orders
        .indexes()
        .iter()
        .map(|index| {
            (
                index.name().to_string(),
                index.is_primary(),
                index.method().map(str::to_string),
            )
        })
        .collect();
    assert_eq!(
        indexes,
        [
            ("primary_key".into(), true, Some("MergeTree".into())),
            (
                "note_bloom".into(),
                false,
                Some("bloom_filter(0.01)".into())
            ),
        ]
    );
    assert_eq!(&*orders.constraints()[0].name(), "positive");

    let recent = shop.relation("recent").unwrap();
    assert_eq!(recent.relation_type(), RelationType::View);
    assert_eq!(recent.definition(), Some("SELECT id FROM shop.orders"));
    assert_eq!(recent.estimated_rows(), None);

    // Functions belong to the database the session started in.
    assert_eq!(shop.routines()[0].arguments(), "x, k, b");
    let other = runtime
        .block_on(connection.introspect_schema("default".into()))
        .unwrap();
    assert!(other.routines().is_empty());

    let search_path = runtime.block_on(connection.search_path()).unwrap();
    assert_eq!(search_path, [Arc::from("shop")]);
}
