//! Firefox's cookie store: plain values in one SQLite table.

use std::path::Path;

use rusqlite::{Connection, Row};

use super::{Cookie, Failure, Read, SameSite};

/// Container and private-browsing cookies carry origin attributes and are
/// not part of the profile's ordinary jar.
pub(super) const COUNT: &str = "select count(*) from moz_cookies where originAttributes = ''";

/// From this schema on, `expiry` is in milliseconds rather than seconds.
const MILLISECOND_EXPIRY_FROM: i64 = 16;

pub(super) fn read(db: &Path) -> Result<Read, Failure> {
    let snapshot = super::snapshot(db)?;
    rows(&snapshot.db).map_err(|_| Failure::ReadFailed)
}

fn rows(db: &Connection) -> rusqlite::Result<Read> {
    let schema: i64 = db.query_row("pragma user_version", [], |row| row.get(0))?;
    let mut statement = db.prepare(
        "select host, name, value, path, expiry, isSecure, isHttpOnly, sameSite \
         from moz_cookies where originAttributes = ''",
    )?;
    let mut rows = statement.query([])?;
    let mut read = Read::default();
    while let Some(row) = rows.next()? {
        if read.full() {
            read.skipped += 1;
            continue;
        }
        // A row of unexpected types is skipped rather than failing the rest.
        match cookie(row, schema) {
            Ok(cookie) => read.push(cookie),
            Err(_) => read.skipped += 1,
        }
    }
    Ok(read)
}

fn cookie(row: &Row, schema: i64) -> rusqlite::Result<Cookie> {
    let flag = |column| {
        Ok::<_, rusqlite::Error>(
            row.get::<_, Option<i64>>(column)?
                .is_some_and(|flag| flag != 0),
        )
    };
    let expiry: i64 = row.get::<_, Option<i64>>(4)?.unwrap_or(0);
    let expires = if schema >= MILLISECOND_EXPIRY_FROM {
        expiry / 1000
    } else {
        expiry
    };
    Ok(Cookie {
        host: row.get(0)?,
        name: row.get(1)?,
        value: row.get(2)?,
        path: row.get(3)?,
        secure: flag(5)?,
        http_only: flag(6)?,
        same_site: SameSite::stored(row.get(7)?),
        expires: (expires > 0).then_some(expires),
    })
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use rusqlite::params;

    /// Firefox's table, with `sameSite` nullable as older schemas left it.
    pub(in super::super) const SCHEMA: &str = "create table moz_cookies (
        id integer primary key, originAttributes text not null default '', name text, value text,
        host text, path text, expiry integer, lastAccessed integer, creationTime integer,
        isSecure integer, isHttpOnly integer, inBrowserElement integer default 0,
        sameSite integer, schemeMap integer default 0)";

    #[test]
    fn expiry_unit_follows_the_schema_version() {
        for (schema, unit) in [(15, 1), (16, 1000)] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("cookies.sqlite");
            let db = Connection::open(&path).unwrap();
            db.execute_batch(SCHEMA).unwrap();
            db.pragma_update(None, "user_version", schema).unwrap();
            let unix = 1_900_000_000_i64;
            for (name, attributes, expiry, secure, http_only, same_site) in [
                ("a", "", unix * unit, 1, 0, Some(0)),
                ("b", "", 0, 0, 1, Some(1)),
                ("c", "", -1, 1, 1, Some(2)),
                ("d", "", 0, 0, 0, None),
                ("e", "", 0, 0, 0, Some(256)),
                // A container tab's cookie is not the profile's.
                ("f", "^userContextId=2", 0, 0, 0, Some(0)),
            ] {
                db.execute(
                    "insert into moz_cookies (name, originAttributes, expiry, isSecure, isHttpOnly,
                     sameSite, value, host, path) values (?1, ?2, ?3, ?4, ?5, ?6, 'v', '.host.test', '/p')",
                    params![name, attributes, expiry, secure, http_only, same_site],
                )
                .unwrap();
            }
            // A row without a name cannot become a cookie.
            db.execute(
                "insert into moz_cookies (host, path, value) values ('h', '/', 'v')",
                [],
            )
            .unwrap();
            assert_eq!(super::super::count(&path, COUNT), Some(6));
            drop(db);

            let read = read(&path).unwrap();
            let attributes: Vec<_> = read
                .cookies
                .iter()
                .map(|c| {
                    (
                        c.name.as_str(),
                        c.expires,
                        c.secure,
                        c.http_only,
                        c.same_site,
                    )
                })
                .collect();
            assert_eq!(
                attributes,
                [
                    ("a", Some(unix), true, false, SameSite::None),
                    ("b", None, false, true, SameSite::Lax),
                    ("c", None, true, true, SameSite::Strict),
                    ("d", None, false, false, SameSite::Unspecified),
                    ("e", None, false, false, SameSite::Unspecified),
                ],
                "schema {schema}"
            );
            assert_eq!(read.skipped, 1);
            let first = &read.cookies[0];
            assert_eq!(
                (first.host.as_str(), first.path.as_str()),
                (".host.test", "/p")
            );
            assert_eq!(first.value, "v");
        }
    }

    #[test]
    fn a_store_without_the_cookie_table_fails() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cookies.sqlite");
        Connection::open(&path)
            .unwrap()
            .execute_batch("create table other (x)")
            .unwrap();
        assert_eq!(read(&path).err(), Some(Failure::ReadFailed));
    }
}
