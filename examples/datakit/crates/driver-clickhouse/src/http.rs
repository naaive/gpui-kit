//! Requests to ClickHouse's HTTP interface.
//!
//! Every statement is one `POST` with the statement as its body. Who is
//! asking travels in the `X-ClickHouse-User` and `X-ClickHouse-Key` headers,
//! so the password never appears in a URL or a log of one; the database,
//! settings and query parameters travel in the query string.

use std::{sync::Arc, time::Duration};

use anyhow::{Context as _, Result};
use futures::StreamExt as _;
use reqwest::Response;

use crate::{error, tsv};

/// How requests reach the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Transport {
    Http,
    /// TLS without checking who is on the other end.
    HttpsUnverified,
    /// TLS with the certificate checked against the system's roots and the
    /// host name.
    Https,
}

/// The endpoint of one data source, and who to be there.
#[derive(Clone)]
pub(crate) struct Http {
    client: reqwest::Client,
    url: Arc<str>,
    user: Arc<str>,
    password: Option<Arc<str>>,
    database: Arc<str>,
}

impl Http {
    pub(crate) fn new(
        transport: Transport,
        host: &str,
        port: u16,
        user: &str,
        password: Option<String>,
        database: &str,
    ) -> Result<Self> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .user_agent("DataKit")
            .danger_accept_invalid_certs(transport == Transport::HttpsUnverified)
            .build()
            .context("HTTP is not available")?;
        let scheme = match transport {
            Transport::Http => "http",
            Transport::HttpsUnverified | Transport::Https => "https",
        };
        // An IPv6 address is bracketed in a URL.
        let host = if host.contains(':') && !host.starts_with('[') {
            format!("[{host}]")
        } else {
            host.to_string()
        };
        Ok(Self {
            client,
            url: format!("{scheme}://{host}:{port}/").into(),
            user: user.into(),
            password: password
                .filter(|password| !password.is_empty())
                .map(Arc::from),
            database: database.into(),
        })
    }

    /// Send `sql` with `parameters` — settings such as `query_id`, and
    /// `param_<name>` values for `{name:Type}` placeholders. A response the
    /// server marks as an error becomes that error.
    pub(crate) async fn send(&self, sql: &str, parameters: &[(&str, &str)]) -> Result<Response> {
        let mut request = self.client.post(&*self.url);
        if !self.user.is_empty() {
            request = request.header("X-ClickHouse-User", &*self.user);
        }
        if let Some(password) = &self.password {
            request = request.header("X-ClickHouse-Key", &**password);
        }
        if !self.database.is_empty() {
            request = request.query(&[("database", &*self.database)]);
        }
        let response = request
            .query(parameters)
            .body(sql.to_string())
            .send()
            .await?;
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status().as_u16();
        let code = header(&response, "X-ClickHouse-Exception-Code");
        let body = response.text().await.unwrap_or_default();
        Err(error::from_response(status, code.as_deref(), &body, sql))
    }

    /// Run `sql` and read its whole result.
    pub(crate) async fn query(&self, sql: &str, parameters: &[(&str, &str)]) -> Result<ResultSet> {
        let mut parameters = parameters.to_vec();
        parameters.push(("default_format", tsv::FORMAT));
        let response = self.send(sql, &parameters).await?;
        ResultSet::from_response(response, sql).await
    }

    /// Stop the statement `query_id`. With `wait`, the future resolves once
    /// it has stopped, not when the server has agreed to stop it.
    pub(crate) async fn kill(&self, query_id: &str, wait: bool) -> Result<()> {
        let sql = format!(
            "KILL QUERY WHERE query_id = '{}' {}",
            query_id.replace('\\', "\\\\").replace('\'', "\\'"),
            if wait { "SYNC" } else { "ASYNC" }
        );
        self.query(&sql, &[]).await?;
        Ok(())
    }
}

/// The value of the response header `name`, when it is text.
pub(crate) fn header(response: &Response, name: &str) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

/// The body of `response`, read line by line as it arrives.
pub(crate) fn body(response: Response, sql: Arc<str>) -> tsv::Body {
    let chunks = response
        .bytes_stream()
        .map(|chunk| chunk.map_err(anyhow::Error::from))
        .boxed();
    tsv::Body::new(chunks, sql)
}

/// A whole result in `TabSeparatedWithNamesAndTypes`, for the driver's own
/// queries: introspection, the server version.
#[derive(Debug, Default)]
pub(crate) struct ResultSet {
    columns: Vec<String>,
    rows: Vec<Vec<Option<String>>>,
}

impl ResultSet {
    pub(crate) async fn from_response(response: Response, sql: &str) -> Result<Self> {
        let mut body = body(response, sql.into());
        let Some(names) = body.next_line().await? else {
            return Ok(Self::default());
        };
        let columns = tsv::fields(&names)
            .into_iter()
            .map(Option::unwrap_or_default)
            .collect();
        // The types are not needed: the driver knows what it asked for.
        body.next_line().await?;
        let mut rows = Vec::new();
        while let Some(line) = body.next_line().await? {
            rows.push(tsv::fields(&line));
        }
        Ok(Self { columns, rows })
    }

    pub(crate) fn records(&self) -> impl Iterator<Item = Record<'_>> {
        self.rows.iter().map(|values| Record {
            columns: &self.columns,
            values,
        })
    }

    /// The first value of the first row.
    pub(crate) fn first_value(&self) -> Option<&str> {
        self.rows.first()?.first()?.as_deref()
    }
}

/// One row of a [`ResultSet`], read by column name.
#[derive(Clone, Copy)]
pub(crate) struct Record<'a> {
    columns: &'a [String],
    values: &'a [Option<String>],
}

impl<'a> Record<'a> {
    /// The value of `column`; `None` when it is `NULL` or the server has no
    /// such column, which an older server may lack.
    pub(crate) fn get(&self, column: &str) -> Option<&'a str> {
        let ix = self.columns.iter().position(|name| name == column)?;
        self.values.get(ix)?.as_deref()
    }

    /// The value of `column`, empty when there is none.
    pub(crate) fn text(&self, column: &str) -> &'a str {
        self.get(column).unwrap_or_default()
    }
}
