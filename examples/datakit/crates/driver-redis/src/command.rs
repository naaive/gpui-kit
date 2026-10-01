//! A command line as `redis-cli` reads it.

use anyhow::{Result, bail};

/// The words of `line`: separated by whitespace, with `"…"` taking the
/// escapes `\"`, `\\`, `\n`, `\r`, `\t` and `\xHH`, and `'…'` only `\'`.
pub(crate) fn split(line: &str) -> Result<Vec<Vec<u8>>> {
    let mut words = Vec::new();
    let mut chars = line.chars().peekable();
    loop {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        let Some(&first) = chars.peek() else {
            return Ok(words);
        };
        let mut word = Vec::new();
        match first {
            '"' => {
                chars.next();
                loop {
                    match chars.next() {
                        None => bail!("A double quote is not closed"),
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some('n') => word.push(b'\n'),
                            Some('r') => word.push(b'\r'),
                            Some('t') => word.push(b'\t'),
                            Some('x') => {
                                let hex: String = chars.by_ref().take(2).collect();
                                match u8::from_str_radix(&hex, 16) {
                                    Ok(byte) => word.push(byte),
                                    Err(_) => bail!("\\x{hex} is not a byte"),
                                }
                            }
                            Some(c) => push(&mut word, c),
                            None => bail!("A double quote is not closed"),
                        },
                        Some(c) => push(&mut word, c),
                    }
                }
            }
            '\'' => {
                chars.next();
                loop {
                    match chars.next() {
                        None => bail!("A single quote is not closed"),
                        Some('\'') => break,
                        Some('\\') if chars.peek() == Some(&'\'') => {
                            chars.next();
                            word.push(b'\'');
                        }
                        Some(c) => push(&mut word, c),
                    }
                }
            }
            _ => {
                while let Some(c) = chars.next_if(|c| !c.is_whitespace()) {
                    push(&mut word, c);
                }
            }
        }
        words.push(word);
    }
}

fn push(word: &mut Vec<u8>, c: char) {
    let mut buffer = [0; 4];
    word.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
}

/// `word` as a command line writes it: bare when it can be, quoted
/// otherwise.
pub(crate) fn quote(word: &str) -> String {
    let bare = !word.is_empty()
        && word
            .chars()
            .all(|c| !c.is_whitespace() && !matches!(c, '"' | '\'' | '\\'));
    if bare {
        return word.to_string();
    }
    let mut quoted = String::from('"');
    for c in word.chars() {
        match c {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

/// Commands that only read, which a read-only data source runs.
pub(crate) const READING: &[&str] = &[
    "BITCOUNT",
    "BITPOS",
    "CLIENT",
    "COMMAND",
    "DATAKIT.LEN",
    "DATAKIT.VALUE",
    "DBSIZE",
    "DUMP",
    "ECHO",
    "EXISTS",
    "GEODIST",
    "GEOHASH",
    "GEOPOS",
    "GEORADIUS_RO",
    "GEORADIUSBYMEMBER_RO",
    "GEOSEARCH",
    "GET",
    "GETBIT",
    "GETRANGE",
    "HEXISTS",
    "HGET",
    "HGETALL",
    "HKEYS",
    "HLEN",
    "HMGET",
    "HRANDFIELD",
    "HSCAN",
    "HSTRLEN",
    "HVALS",
    "INFO",
    "KEYS",
    "LASTSAVE",
    "LINDEX",
    "LLEN",
    "LPOS",
    "LRANGE",
    "MEMORY",
    "MGET",
    "OBJECT",
    "PFCOUNT",
    "PING",
    "PTTL",
    "RANDOMKEY",
    "ROLE",
    "SCAN",
    "SCARD",
    "SDIFF",
    "SELECT",
    "SINTER",
    "SINTERCARD",
    "SISMEMBER",
    "SMEMBERS",
    "SMISMEMBER",
    "SRANDMEMBER",
    "SSCAN",
    "STRLEN",
    "SUBSTR",
    "SUNION",
    "TIME",
    "TTL",
    "TYPE",
    "XINFO",
    "XLEN",
    "XPENDING",
    "XRANGE",
    "XREAD",
    "XREVRANGE",
    "ZCARD",
    "ZCOUNT",
    "ZDIFF",
    "ZINTER",
    "ZLEXCOUNT",
    "ZMSCORE",
    "ZRANDMEMBER",
    "ZRANGE",
    "ZRANGEBYLEX",
    "ZRANGEBYSCORE",
    "ZRANK",
    "ZREVRANGE",
    "ZREVRANGEBYLEX",
    "ZREVRANGEBYSCORE",
    "ZREVRANK",
    "ZSCAN",
    "ZSCORE",
    "ZUNION",
];

/// Every command completion offers, the reading ones among them.
pub(crate) const COMMANDS: &[&str] = &[
    "APPEND",
    "BITCOUNT",
    "BITPOS",
    "CLIENT",
    "COMMAND",
    "CONFIG",
    "COPY",
    "DBSIZE",
    "DECR",
    "DECRBY",
    "DEL",
    "DUMP",
    "ECHO",
    "EVAL",
    "EVALSHA",
    "EXEC",
    "EXISTS",
    "EXPIRE",
    "EXPIREAT",
    "FLUSHALL",
    "FLUSHDB",
    "GEOADD",
    "GEODIST",
    "GEOHASH",
    "GEOPOS",
    "GEOSEARCH",
    "GET",
    "GETBIT",
    "GETDEL",
    "GETEX",
    "GETRANGE",
    "GETSET",
    "HDEL",
    "HEXISTS",
    "HGET",
    "HGETALL",
    "HINCRBY",
    "HINCRBYFLOAT",
    "HKEYS",
    "HLEN",
    "HMGET",
    "HMSET",
    "HRANDFIELD",
    "HSCAN",
    "HSET",
    "HSETNX",
    "HSTRLEN",
    "HVALS",
    "INCR",
    "INCRBY",
    "INCRBYFLOAT",
    "INFO",
    "KEYS",
    "LASTSAVE",
    "LINDEX",
    "LINSERT",
    "LLEN",
    "LMOVE",
    "LPOP",
    "LPOS",
    "LPUSH",
    "LPUSHX",
    "LRANGE",
    "LREM",
    "LSET",
    "LTRIM",
    "MEMORY",
    "MGET",
    "MOVE",
    "MSET",
    "MSETNX",
    "MULTI",
    "OBJECT",
    "PERSIST",
    "PEXPIRE",
    "PFADD",
    "PFCOUNT",
    "PFMERGE",
    "PING",
    "PSETEX",
    "PTTL",
    "PUBLISH",
    "RANDOMKEY",
    "RENAME",
    "RENAMENX",
    "RESTORE",
    "ROLE",
    "RPOP",
    "RPUSH",
    "RPUSHX",
    "SADD",
    "SCAN",
    "SCARD",
    "SDIFF",
    "SDIFFSTORE",
    "SELECT",
    "SET",
    "SETBIT",
    "SETEX",
    "SETNX",
    "SETRANGE",
    "SINTER",
    "SINTERSTORE",
    "SISMEMBER",
    "SMEMBERS",
    "SMISMEMBER",
    "SMOVE",
    "SPOP",
    "SRANDMEMBER",
    "SREM",
    "SSCAN",
    "STRLEN",
    "SUNION",
    "SUNIONSTORE",
    "TIME",
    "TOUCH",
    "TTL",
    "TYPE",
    "UNLINK",
    "XADD",
    "XDEL",
    "XINFO",
    "XLEN",
    "XRANGE",
    "XREAD",
    "XREVRANGE",
    "XTRIM",
    "ZADD",
    "ZCARD",
    "ZCOUNT",
    "ZINCRBY",
    "ZLEXCOUNT",
    "ZMSCORE",
    "ZPOPMAX",
    "ZPOPMIN",
    "ZRANDMEMBER",
    "ZRANGE",
    "ZRANGEBYLEX",
    "ZRANGEBYSCORE",
    "ZRANK",
    "ZREM",
    "ZREVRANGE",
    "ZREVRANGEBYSCORE",
    "ZREVRANK",
    "ZSCAN",
    "ZSCORE",
];

/// Whether `line` only reads: its command is one that does, or it reads
/// the server's configuration.
pub(crate) fn reads_only(line: &str) -> bool {
    let Ok(words) = split(line) else {
        return false;
    };
    let word = |ix: usize| {
        words
            .get(ix)
            .map(|word| String::from_utf8_lossy(word).to_uppercase())
    };
    match word(0) {
        None => true,
        Some(command) if command == "CONFIG" => word(1).as_deref() == Some("GET"),
        Some(command) => READING.contains(&command.as_str()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        split(line)
            .unwrap()
            .into_iter()
            .map(|word| String::from_utf8(word).unwrap())
            .collect()
    }

    #[test]
    fn lines_split_as_redis_cli_splits_them() {
        assert_eq!(
            words("  SET user:1  \"Ada \\\"L\\\"\\n\" "),
            ["SET", "user:1", "Ada \"L\"\n"]
        );
        assert_eq!(
            words("hset h 'it\\'s' \"\\x41\""),
            ["hset", "h", "it's", "A"]
        );
        assert!(split("get \"open").is_err());
        assert!(words("").is_empty());
    }

    #[test]
    fn quoting_round_trips() {
        for word in ["user:1", "two words", "say \"hi\"", "", "back\\slash"] {
            assert_eq!(words(&quote(word)), [word]);
        }
    }

    #[test]
    fn reading_commands_are_told_apart() {
        assert!(reads_only("get user:1"));
        assert!(reads_only("CONFIG GET maxmemory"));
        assert!(!reads_only("config set maxmemory 1"));
        assert!(!reads_only("SET a b"));
        assert!(!reads_only("FLUSHALL"));
    }
}
