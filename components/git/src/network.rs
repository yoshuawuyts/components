use crate::{
    CloneResult, Component, Error, FetchResult, Guest, NetworkCredentials, NetworkError,
    NetworkOptions, Reference, branch_name, open_bare, repository_error, update_branch,
    validate_repository_path,
};
use gix::bstr::ByteSlice;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Cursor,
    path::Path,
    sync::atomic::AtomicBool,
};
use wasi::http::{
    outgoing_handler,
    types::{Fields, IncomingBody, Method, OutgoingBody, OutgoingRequest, Scheme},
};
use wasi::io::streams::StreamError;

const MAX_RESPONSE: u64 = 256 * 1024 * 1024;
const MAX_REFERENCES: u32 = 100_000;
const MAX_PACK_OBJECTS: u32 = 1_000_000;
const MAX_PACK_LINE: usize = 65_520;
const MAX_PACKET_COUNT: usize = 200_000;

#[derive(Clone, Debug, Eq, PartialEq)]
struct HttpUrl {
    scheme: String,
    authority: String,
    host: String,
    base_path: String,
    original: String,
}

#[derive(Clone, Copy)]
struct HttpRequest<'a> {
    endpoint: &'a str,
    method: &'a Method,
    accept: &'a str,
    content_type: Option<&'a str>,
    credentials: Option<&'a NetworkCredentials>,
    body: Option<&'a [u8]>,
    max_bytes: usize,
}

struct PackData(Vec<u8>);

#[derive(Debug)]
struct Advertisement {
    refs: BTreeMap<String, gix::ObjectId>,
    head: Option<String>,
    head_oid: Option<gix::ObjectId>,
    side_band_64k: bool,
    ofs_delta: bool,
}

fn network_error(error: NetworkError) -> Error {
    Error::Network(error)
}

fn transport_error() -> Error {
    network_error(NetworkError::TransportFailure)
}

fn malformed(message: &str) -> Error {
    network_error(NetworkError::MalformedResponse(message.into()))
}

fn parse_url(input: &str) -> Result<HttpUrl, Error> {
    if input.len() > 8192 {
        return Err(Error::InvalidInput("remote URL exceeds 8192 bytes".into()));
    }
    let (scheme, remainder) = input
        .split_once("://")
        .ok_or_else(|| Error::InvalidInput("URL must use http:// or https://".into()))?;
    if scheme != "http" && scheme != "https" {
        return Err(Error::Unsupported(
            "only http and https remote URLs are supported".into(),
        ));
    }
    if input.contains(['?', '#', '\\', '"', '\'', '\r', '\n', '\0'])
        || input.chars().any(char::is_control)
    {
        return Err(Error::InvalidInput(
            "URL queries, fragments, quotes, backslashes, and control characters are unsupported"
                .into(),
        ));
    }
    let end_authority = remainder.find('/').unwrap_or(remainder.len());
    let authority = remainder
        .get(..end_authority)
        .ok_or_else(|| Error::InvalidInput("invalid URL authority boundary".into()))?;
    if authority.is_empty() || authority.contains('@') || authority.contains('%') {
        return Err(Error::InvalidInput(
            "URL must have a hostname and must not contain userinfo".into(),
        ));
    }
    let (host, port) = if authority.starts_with('[') {
        let close = authority
            .find(']')
            .ok_or_else(|| Error::InvalidInput("invalid IPv6 URL authority".into()))?;
        let host = authority
            .get(1..close)
            .ok_or_else(|| Error::InvalidInput("invalid IPv6 URL authority".into()))?;
        let suffix = authority
            .get(close + 1..)
            .ok_or_else(|| Error::InvalidInput("invalid IPv6 URL authority".into()))?;
        if host.parse::<std::net::Ipv6Addr>().is_err()
            || (!suffix.is_empty() && !suffix.starts_with(':'))
        {
            return Err(Error::InvalidInput("invalid IPv6 URL authority".into()));
        }
        (host, suffix.strip_prefix(':'))
    } else {
        let mut parts = authority.split(':');
        let host = parts.next().unwrap_or_default();
        let port = parts.next();
        if parts.next().is_some() {
            return Err(Error::InvalidInput(
                "IPv6 URL hosts must use square brackets".into(),
            ));
        }
        (host, port)
    };
    validate_host(host)?;
    if authority.len() > 255 || raw_host_path_len(remainder, end_authority) > 4096 {
        return Err(Error::InvalidInput(
            "remote URL authority or path is too long".into(),
        ));
    }
    if let Some(port) = port
        && (port.is_empty() || port.parse::<u16>().map_or(true, |port| port == 0))
    {
        return Err(Error::InvalidInput("invalid URL port".into()));
    }
    let raw_path = remainder
        .get(end_authority..)
        .ok_or_else(|| Error::InvalidInput("invalid URL path boundary".into()))?;
    if raw_path.bytes().any(|b| b.is_ascii_whitespace()) {
        return Err(Error::InvalidInput(
            "URL path must percent-encode whitespace".into(),
        ));
    }
    let base_path = raw_path.trim_end_matches('/').to_owned();
    Ok(HttpUrl {
        scheme: scheme.to_owned(),
        authority: authority.to_owned(),
        host: host.to_ascii_lowercase(),
        base_path,
        original: input.to_owned(),
    })
}

fn raw_host_path_len(remainder: &str, end_authority: usize) -> usize {
    remainder.len().saturating_sub(end_authority)
}

fn validate_host(host: &str) -> Result<(), Error> {
    if host.is_empty() || !host.is_ascii() {
        return Err(Error::InvalidInput("invalid URL hostname".into()));
    }
    if host.contains(':') {
        host.parse::<std::net::Ipv6Addr>()
            .map_err(|_| Error::InvalidInput("invalid IPv6 hostname".into()))?;
        return Ok(());
    }
    if host.len() > 253
        || host.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
    {
        return Err(Error::InvalidInput("invalid URL hostname".into()));
    }
    Ok(())
}

fn validate_remote_name(name: &str) -> Result<(), Error> {
    if name.is_empty()
        || name.len() > 128
        || name == "."
        || name == ".."
        || name.starts_with('-')
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(Error::InvalidInput(
            "remote name must be 1..=128 ASCII letters, digits, dots, underscores, or hyphens"
                .into(),
        ));
    }
    Ok(())
}

fn validate_options(options: &NetworkOptions, url: &HttpUrl) -> Result<usize, Error> {
    if options.max_response_bytes == 0
        || options.max_response_bytes > MAX_RESPONSE
        || options.max_pack_bytes == 0
        || options.max_pack_bytes > MAX_RESPONSE
        || options.max_pack_bytes > options.max_response_bytes
        || options.max_refs == 0
        || options.max_refs > MAX_REFERENCES
        || options.allowed_hosts.is_empty()
        || options.allowed_hosts.len() > 128
    {
        return Err(Error::InvalidInput(
            "network bounds must be response/pack 1..=268435456 bytes, refs 1..=100000, and 1..=128 allowed hosts"
                .into(),
        ));
    }
    let mut found = false;
    for host in &options.allowed_hosts {
        let normalized = host
            .strip_prefix('[')
            .and_then(|value| value.strip_suffix(']'))
            .unwrap_or(host)
            .to_ascii_lowercase();
        if normalized == "*" || validate_host(&normalized).is_err() {
            return Err(Error::InvalidInput(
                "allowed-hosts must contain exact valid hostnames without wildcards".into(),
            ));
        }
        found |= normalized == url.host;
    }
    if !found {
        return Err(Error::InvalidInput(
            "remote hostname is not present in allowed-hosts".into(),
        ));
    }
    if let Some(credentials) = &options.credentials {
        validate_credentials(credentials)?;
    }
    usize::try_from(options.max_response_bytes)
        .map_err(|_| Error::InvalidInput("response limit does not fit this target".into()))
}

fn validate_credentials(credentials: &NetworkCredentials) -> Result<(), Error> {
    let basic = credentials.username.is_some() || credentials.password.is_some();
    let bearer = credentials.bearer_token.is_some();
    if basic == bearer
        || (basic && (credentials.username.is_none() || credentials.password.is_none()))
    {
        return Err(Error::InvalidInput(
            "provide either a complete Basic credential pair or a bearer token".into(),
        ));
    }
    let valid_value = |value: &str| !value.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0);
    if let Some(username) = &credentials.username
        && (username.len() > 4096 || !valid_value(username) || username.contains(':'))
    {
        return Err(Error::InvalidInput("invalid Basic username".into()));
    }
    if let Some(password) = &credentials.password
        && (password.len() > 4096 || !valid_value(password))
    {
        return Err(Error::InvalidInput("invalid Basic password".into()));
    }
    if let Some(token) = &credentials.bearer_token
        && (token.is_empty()
            || token.len() > 8192
            || !valid_value(token)
            || !valid_bearer_token(token))
    {
        return Err(Error::InvalidInput("invalid bearer token".into()));
    }
    Ok(())
}

fn valid_bearer_token(token: &str) -> bool {
    let mut padding = false;
    for byte in token.bytes() {
        if byte == b'=' {
            padding = true;
        } else if padding
            || !byte.is_ascii_alphanumeric()
                && !matches!(byte, b'-' | b'.' | b'_' | b'~' | b'+' | b'/')
        {
            return false;
        }
    }
    true
}

fn base64(input: &[u8]) -> Result<String, Error> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let a = *chunk
            .first()
            .ok_or_else(|| Error::InvalidInput("cannot encode empty base64 chunk".into()))?;
        let b = *chunk.get(1).unwrap_or(&0);
        let c = *chunk.get(2).unwrap_or(&0);
        for index in [
            a >> 2,
            ((a & 0x03) << 4) | (b >> 4),
            ((b & 0x0f) << 2) | (c >> 6),
            c & 0x3f,
        ] {
            let character = TABLE
                .get(usize::from(index))
                .copied()
                .map(char::from)
                .ok_or_else(|| Error::InvalidInput("base64 index is out of range".into()))?;
            out.push(character);
        }
        if chunk.len() < 3 {
            let padding = if chunk.len() == 1 { 2 } else { 1 };
            out.truncate(out.len() - padding);
            out.extend(std::iter::repeat_n('=', padding));
        }
    }
    Ok(out)
}

fn request_headers(
    accept: &str,
    content_type: Option<&str>,
    credentials: Option<&NetworkCredentials>,
) -> Result<Fields, Error> {
    let headers = Fields::new();
    let append = |name: &str, value: &[u8]| {
        headers
            .append(&name.to_owned(), &value.to_vec())
            .map_err(|_| transport_error())
    };
    append("accept", accept.as_bytes())?;
    if let Some(content_type) = content_type {
        append("content-type", content_type.as_bytes())?;
    }
    if let Some(credentials) = credentials {
        let value = if let (Some(username), Some(password)) =
            (&credentials.username, &credentials.password)
        {
            let pair = format!("{username}:{password}");
            format!("Basic {}", base64(pair.as_bytes())?)
        } else if let Some(token) = &credentials.bearer_token {
            format!("Bearer {token}")
        } else {
            return Err(Error::InvalidInput("incomplete credentials".into()));
        };
        append("authorization", value.as_bytes())?;
    }
    Ok(headers)
}

fn http_request(url: &HttpUrl, spec: HttpRequest<'_>) -> Result<(u16, String, Vec<u8>), Error> {
    let headers = request_headers(spec.accept, spec.content_type, spec.credentials)?;
    if let Some(body) = spec.body {
        headers
            .append(
                &"content-length".to_owned(),
                &body.len().to_string().into_bytes(),
            )
            .map_err(|_| transport_error())?;
    }
    let outgoing_request = OutgoingRequest::new(headers);
    outgoing_request
        .set_method(spec.method)
        .map_err(|()| transport_error())?;
    let scheme = if url.scheme == "http" {
        Scheme::Http
    } else {
        Scheme::Https
    };
    outgoing_request
        .set_scheme(Some(&scheme))
        .map_err(|()| transport_error())?;
    outgoing_request
        .set_authority(Some(&url.authority))
        .map_err(|()| transport_error())?;
    outgoing_request
        .set_path_with_query(Some(spec.endpoint))
        .map_err(|()| transport_error())?;
    if let Some(body) = spec.body {
        let outgoing_body = outgoing_request.body().map_err(|()| transport_error())?;
        let stream = outgoing_body.write().map_err(|()| transport_error())?;
        for chunk in body.chunks(64 * 1024) {
            stream
                .blocking_write_and_flush(chunk)
                .map_err(|_| transport_error())?;
        }
        drop(stream);
        OutgoingBody::finish(outgoing_body, None).map_err(|_| transport_error())?;
    }
    let future = outgoing_handler::handle(outgoing_request, None).map_err(|_| transport_error())?;
    let pollable = future.subscribe();
    pollable.block();
    drop(pollable);
    let Some(Ok(Ok(response))) = future.get() else {
        return Err(transport_error());
    };
    let status = response.status();
    if (300..400).contains(&status) {
        return Err(network_error(NetworkError::HttpStatus(status)));
    }
    if status == 401 || status == 403 {
        return Err(network_error(NetworkError::AuthenticationRequired));
    }
    if status != 200 {
        return Err(network_error(NetworkError::HttpStatus(status)));
    }
    let mut content_type = String::new();
    let response_headers = response.headers();
    for (name, value) in response_headers.entries() {
        if name.eq_ignore_ascii_case("content-type") {
            content_type = String::from_utf8(value)
                .map_err(|_| malformed("HTTP Content-Type is not ASCII"))?;
            break;
        }
    }
    drop(response_headers);
    let incoming_body = response.consume().map_err(|()| transport_error())?;
    let stream = incoming_body.stream().map_err(|()| transport_error())?;
    let mut bytes = Vec::new();
    loop {
        match stream.blocking_read(64 * 1024) {
            Ok(chunk) if chunk.is_empty() => break,
            Ok(chunk) => {
                if bytes.len().saturating_add(chunk.len()) > spec.max_bytes {
                    return Err(network_error(NetworkError::ResponseTooLarge));
                }
                bytes.extend_from_slice(&chunk);
            }
            Err(StreamError::Closed) => break,
            Err(StreamError::LastOperationFailed(_)) => return Err(transport_error()),
        }
    }
    drop(stream);
    let trailers = IncomingBody::finish(incoming_body);
    let pollable = trailers.subscribe();
    pollable.block();
    drop(pollable);
    match trailers.get() {
        Some(Ok(Ok(_))) => {}
        _ => return Err(transport_error()),
    }
    Ok((status, content_type, bytes))
}

fn check_content_type(actual: &str, expected: &str) -> Result<(), Error> {
    if actual
        .split(';')
        .next()
        .is_some_and(|value| value.trim().eq_ignore_ascii_case(expected))
    {
        Ok(())
    } else {
        Err(malformed("unexpected smart-HTTP Content-Type"))
    }
}

#[derive(Clone, Copy, Debug)]
enum Packet<'a> {
    Data(&'a [u8]),
    Flush,
    Delimiter,
    ResponseEnd,
}

fn visit_packets(
    input: &[u8],
    mut visitor: impl FnMut(Packet<'_>) -> Result<(), Error>,
) -> Result<usize, Error> {
    let mut offset = 0;
    let mut count = 0;
    while offset < input.len() {
        count += 1;
        if count > MAX_PACKET_COUNT {
            return Err(malformed("too many packet lines"));
        }
        let header = input
            .get(offset..offset + 4)
            .ok_or_else(|| malformed("truncated packet-line header"))?;
        let len = usize::from_str_radix(
            std::str::from_utf8(header).map_err(|_| malformed("invalid packet-line header"))?,
            16,
        )
        .map_err(|_| malformed("invalid packet-line header"))?;
        offset += 4;
        match len {
            0 => visitor(Packet::Flush)?,
            1 => visitor(Packet::Delimiter)?,
            2 => visitor(Packet::ResponseEnd)?,
            3 => return Err(malformed("invalid packet-line length")),
            _ => {
                if !(4..=MAX_PACK_LINE).contains(&len) {
                    return Err(malformed("packet-line exceeds protocol bounds"));
                }
                let end = offset
                    .checked_add(len - 4)
                    .filter(|end| *end <= input.len())
                    .ok_or_else(|| malformed("truncated packet-line data"))?;
                visitor(Packet::Data(
                    input
                        .get(offset..end)
                        .ok_or_else(|| malformed("truncated packet-line data"))?,
                ))?;
                offset = end;
            }
        }
    }
    Ok(count)
}

fn parse_advertisement(input: &[u8], max_refs: u32) -> Result<Advertisement, Error> {
    let mut service = false;
    let mut service_flush = false;
    let mut refs = BTreeMap::new();
    let mut head = None;
    let mut head_oid = None;
    let mut caps = BTreeSet::new();
    let mut first_ref = true;
    let mut finished = false;
    visit_packets(input, |packet| {
        if finished {
            return Err(malformed("data follows advertisement terminator"));
        }
        match packet {
            Packet::Data(data) if !service => {
                if data != b"# service=git-upload-pack\n" {
                    return Err(malformed("invalid upload-pack service announcement"));
                }
                service = true;
            }
            Packet::Flush if service && !service_flush => service_flush = true,
            Packet::Data(_) | Packet::Delimiter | Packet::ResponseEnd if !service_flush => {
                return Err(malformed("missing service announcement flush"));
            }
            Packet::Data(data) => {
                if data.starts_with(b"version ") {
                    return Err(Error::Unsupported(
                        "Git protocol v2 is not supported".into(),
                    ));
                }
                let line = data.strip_suffix(b"\n").unwrap_or(data);
                if line.is_empty() || line.contains(&0) && !first_ref {
                    return Err(malformed("invalid reference advertisement line"));
                }
                let line = if first_ref {
                    if let Some(nul) = line.iter().position(|byte| *byte == 0) {
                        let caps_bytes = line
                            .get(nul + 1..)
                            .ok_or_else(|| malformed("invalid capability boundary"))?;
                        let caps_text = std::str::from_utf8(caps_bytes)
                            .map_err(|_| malformed("non-ASCII capability list"))?;
                        caps.extend(caps_text.split_ascii_whitespace().map(str::to_owned));
                        line.get(..nul)
                            .ok_or_else(|| malformed("invalid capability boundary"))?
                    } else {
                        line
                    }
                } else {
                    line
                };
                let separator = line
                    .iter()
                    .position(|byte| *byte == b' ')
                    .ok_or_else(|| malformed("reference line has no object ID separator"))?;
                let oid_bytes = line
                    .get(..separator)
                    .ok_or_else(|| malformed("invalid reference separator"))?;
                let name_bytes = line
                    .get(separator + 1..)
                    .ok_or_else(|| malformed("invalid reference separator"))?;
                if oid_bytes.len() != 40 || !oid_bytes.iter().all(u8::is_ascii_hexdigit) {
                    return Err(malformed("reference does not use a full SHA-1 object ID"));
                }
                let name = std::str::from_utf8(name_bytes)
                    .map_err(|_| malformed("reference name is not UTF-8"))?;
                if name == "capabilities^{}" {
                    if !oid_bytes.iter().all(|byte| *byte == b'0') {
                        return Err(malformed(
                            "empty-repository capability line has a nonzero ID",
                        ));
                    }
                    first_ref = false;
                    return Ok(());
                }
                if name.ends_with("^{}") {
                    first_ref = false;
                    return Ok(());
                }
                if name != "HEAD"
                    && gix::validate::reference::name(name.as_bytes().as_bstr()).is_err()
                {
                    return Err(malformed("invalid advertised reference name"));
                }
                let oid = gix::ObjectId::from_hex(oid_bytes)
                    .map_err(|_| malformed("invalid advertised object ID"))?;
                if name == "HEAD" {
                    if !oid_bytes.iter().all(|byte| *byte == b'0') {
                        head_oid = Some(oid);
                    }
                } else if !oid_bytes.iter().all(|byte| *byte == b'0') {
                    if refs.insert(name.to_owned(), oid).is_some() {
                        return Err(malformed("duplicate advertised reference"));
                    }
                    if refs.len() > max_refs as usize {
                        return Err(network_error(NetworkError::ResponseTooLarge));
                    }
                }
                first_ref = false;
            }
            Packet::Flush if service_flush => finished = true,
            Packet::ResponseEnd => finished = true,
            Packet::Delimiter => return Err(malformed("unexpected advertisement delimiter")),
            Packet::Flush => return Err(malformed("invalid advertisement flush")),
        }
        Ok(())
    })?;
    if !service || !service_flush {
        return Err(malformed("incomplete service advertisement"));
    }
    if let Some(target) = caps
        .iter()
        .find_map(|cap| cap.strip_prefix("symref=HEAD:").map(str::to_owned))
    {
        if !target.starts_with("refs/heads/")
            || gix::validate::reference::branch_name(target.as_bytes().as_bstr()).is_err()
        {
            return Err(malformed("invalid advertised HEAD target"));
        }
        head = Some(target);
    }
    let side_band_64k = caps.contains("side-band-64k");
    if !refs.is_empty() && !side_band_64k {
        return Err(Error::Unsupported(
            "remote does not advertise side-band-64k".into(),
        ));
    }
    let ofs_delta = caps.contains("ofs-delta");
    Ok(Advertisement {
        refs,
        head,
        head_oid,
        side_band_64k,
        ofs_delta,
    })
}

fn pkt_line(data: &[u8], output: &mut Vec<u8>) -> Result<(), Error> {
    let length = data
        .len()
        .checked_add(4)
        .filter(|length| *length <= MAX_PACK_LINE)
        .ok_or_else(|| Error::InvalidInput("upload-pack request packet is too large".into()))?;
    output.extend_from_slice(format!("{length:04x}").as_bytes());
    output.extend_from_slice(data);
    Ok(())
}

fn upload_pack_request(
    advertisement: &Advertisement,
    repo: Option<&gix::Repository>,
    max_bytes: usize,
) -> Result<Vec<u8>, Error> {
    let wants: Vec<_> = advertisement
        .refs
        .values()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if wants.is_empty() {
        return Ok(Vec::new());
    }
    let mut request = Vec::new();
    for (index, id) in wants.iter().enumerate() {
        let mut line = format!("want {id}");
        if index == 0 {
            line.push_str(" side-band-64k");
            if advertisement.ofs_delta {
                line.push_str(" ofs-delta");
            }
            line.push_str(" agent=components-git/0.1.0");
        }
        line.push('\n');
        pkt_line(line.as_bytes(), &mut request)?;
    }
    request.extend_from_slice(b"0000");
    if let Some(repo) = repo {
        let platform = repo.references().map_err(repository_error)?;
        let mut count = 0;
        for reference in platform.all().map_err(repository_error)? {
            let reference = reference.map_err(repository_error)?;
            let name = reference.name().as_bstr();
            if !name.starts_with(b"refs/heads/") && !name.starts_with(b"refs/remotes/") {
                continue;
            }
            if let Some(id) = reference.try_id() {
                let commit = repo
                    .find_object(id)
                    .map_err(repository_error)?
                    .peel_to_kind(gix::objs::Kind::Commit)
                    .map_err(repository_error)?;
                pkt_line(format!("have {}\n", commit.id).as_bytes(), &mut request)?;
                count += 1;
                if count > MAX_REFERENCES {
                    return Err(Error::Unsupported("too many local references".into()));
                }
            }
        }
    }
    pkt_line(b"done\n", &mut request)?;
    if request.len() > max_bytes {
        return Err(Error::InvalidInput(
            "upload-pack request exceeds bound".into(),
        ));
    }
    Ok(request)
}

fn unpack_sideband(input: &[u8], max_pack: usize) -> Result<Vec<u8>, Error> {
    let mut pack = Vec::new();
    let mut finished = false;
    visit_packets(input, |packet| {
        if finished {
            return Err(malformed("data follows upload-pack response terminator"));
        }
        match packet {
            Packet::Data(data) if data.first() == Some(&1) => {
                let payload = data
                    .get(1..)
                    .ok_or_else(|| malformed("invalid side-band packet"))?;
                if pack.len().saturating_add(payload.len()) > max_pack {
                    return Err(network_error(NetworkError::ResponseTooLarge));
                }
                pack.extend_from_slice(payload);
            }
            Packet::Data(data) if data.first() == Some(&2) => {}
            Packet::Data(data) if data.first() == Some(&3) => {
                return Err(malformed("remote reported an upload-pack error"));
            }
            Packet::Data(data) if data.starts_with(b"NAK\n") || data.starts_with(b"ACK ") => {}
            Packet::Data(_) => return Err(malformed("invalid side-band packet")),
            Packet::Flush | Packet::ResponseEnd => finished = true,
            Packet::Delimiter => return Err(malformed("unexpected upload-pack delimiter")),
        }
        Ok(())
    })?;
    if !finished || pack.len() < 32 || !pack.starts_with(b"PACK") {
        return Err(malformed("upload-pack response has no complete pack"));
    }
    Ok(pack)
}

fn endpoint(url: &HttpUrl, suffix: &str) -> String {
    format!("{}/{suffix}", url.base_path)
}

fn get_advertisement(
    url: &HttpUrl,
    options: &NetworkOptions,
    max_bytes: usize,
) -> Result<Advertisement, Error> {
    let path = endpoint(url, "info/refs?service=git-upload-pack");
    let (_, content_type, body) = http_request(
        url,
        HttpRequest {
            endpoint: &path,
            method: &Method::Get,
            accept: "application/x-git-upload-pack-advertisement",
            content_type: None,
            credentials: options.credentials.as_ref(),
            body: None,
            max_bytes,
        },
    )?;
    check_content_type(&content_type, "application/x-git-upload-pack-advertisement")?;
    parse_advertisement(&body, options.max_refs)
}

fn request_pack(
    url: &HttpUrl,
    options: &NetworkOptions,
    max_response: usize,
    advertisement: &Advertisement,
    repo: Option<&gix::Repository>,
) -> Result<Vec<u8>, Error> {
    if advertisement.refs.is_empty() {
        return Ok(Vec::new());
    }
    if !advertisement.side_band_64k {
        return Err(Error::Unsupported(
            "remote lacks required side-band-64k capability".into(),
        ));
    }
    let request = upload_pack_request(advertisement, repo, max_response)?;
    if request.is_empty() {
        return Ok(Vec::new());
    }
    let path = endpoint(url, "git-upload-pack");
    let (_, content_type, body) = http_request(
        url,
        HttpRequest {
            endpoint: &path,
            method: &Method::Post,
            accept: "application/x-git-upload-pack-result",
            content_type: Some("application/x-git-upload-pack-request"),
            credentials: options.credentials.as_ref(),
            body: Some(&request),
            max_bytes: max_response,
        },
    )?;
    check_content_type(&content_type, "application/x-git-upload-pack-result")?;
    let max_pack = usize::try_from(options.max_pack_bytes)
        .map_err(|_| Error::InvalidInput("pack limit does not fit this target".into()))?;
    unpack_sideband(&body, max_pack)
}

fn install_pack(repo: &gix::Repository, pack: &[u8]) -> Result<(), Error> {
    if pack.is_empty() {
        return Ok(());
    }
    if pack.get(..4) != Some(b"PACK") {
        return Err(malformed("pack header is missing"));
    }
    let version = u32::from_be_bytes(
        pack.get(4..8)
            .ok_or_else(|| malformed("invalid pack version"))?
            .try_into()
            .map_err(|_| malformed("invalid pack version"))?,
    );
    if !matches!(version, 2 | 3) {
        return Err(malformed("unsupported pack version"));
    }
    let object_count = u32::from_be_bytes(
        pack.get(8..12)
            .ok_or_else(|| malformed("invalid pack object count"))?
            .try_into()
            .map_err(|_| malformed("invalid pack object count"))?,
    );
    if object_count > MAX_PACK_OBJECTS {
        return Err(network_error(NetworkError::ResponseTooLarge));
    }
    let hash_len = repo.object_hash().len_in_bytes();
    let trailer_start = pack
        .len()
        .checked_sub(hash_len)
        .ok_or_else(|| malformed("pack is shorter than its checksum"))?;
    let mut expected = gix::ObjectId::null(repo.object_hash());
    expected.as_mut_slice().copy_from_slice(
        pack.get(trailer_start..)
            .ok_or_else(|| malformed("pack checksum is truncated"))?,
    );
    let mut hasher = gix::hash::hasher(repo.object_hash());
    hasher.update(
        pack.get(..trailer_start)
            .ok_or_else(|| malformed("pack checksum is truncated"))?,
    );
    let actual = hasher
        .try_finalize()
        .map_err(|_| malformed("unable to compute pack checksum"))?;
    expected
        .verify(&actual)
        .map_err(|_| malformed("pack checksum mismatch"))?;

    let directory = repo.git_dir().join("objects/pack");
    std::fs::create_dir_all(&directory).map_err(repository_error)?;
    let mut entries = gix_pack::data::input::BytesToEntriesIter::new_from_header(
        Cursor::new(pack),
        gix_pack::data::input::Mode::Verify,
        gix_pack::data::input::EntryDataMode::Crc32,
        repo.object_hash(),
    )
    .map_err(repository_error)?;
    let version = entries.version();
    let mut index_data = Vec::new();
    let pack_bytes = PackData(pack.to_vec());
    let outcome = gix_pack::index::write_data_iter_to_stream(
        gix_pack::index::Version::default(),
        || Ok((resolve_pack_entry, pack_bytes)),
        &mut entries,
        Some(1),
        &mut gix::progress::Discard,
        &mut index_data,
        &AtomicBool::new(false),
        repo.object_hash(),
        Some(
            usize::try_from(MAX_RESPONSE)
                .map_err(|_| Error::InvalidInput("pack limit does not fit this target".into()))?,
        ),
        version,
    )
    .map_err(repository_error)?;
    if outcome.data_hash != actual {
        return Err(malformed("pack indexer verified a different pack checksum"));
    }

    let pack_name = format!("pack-{}.pack", outcome.data_hash);
    let index_name = format!("pack-{}.idx", outcome.data_hash);
    write_pack_files(
        &directory.join(pack_name),
        &directory.join(index_name),
        pack,
        &index_data,
    )?;
    Ok(())
}

fn resolve_pack_entry(range: std::ops::Range<u64>, bytes: &PackData) -> Option<&[u8]> {
    let start = usize::try_from(range.start).ok()?;
    let end = usize::try_from(range.end).ok()?;
    bytes.0.get(start..end)
}

fn write_pack_files(
    pack_path: &Path,
    index_path: &Path,
    pack: &[u8],
    index: &[u8],
) -> Result<(), Error> {
    use std::io::Write;

    if pack_path.exists() && index_path.exists() {
        if std::fs::read(pack_path).map_err(repository_error)? == pack
            && std::fs::read(index_path).map_err(repository_error)? == index
        {
            return Ok(());
        }
        return Err(Error::Repository(
            "existing pack or index does not match the verified data".into(),
        ));
    }
    let pack_lock_path = pack_path.with_extension("pack.lock");
    let index_lock_path = index_path.with_extension("idx.lock");
    let mut pack_lock = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&pack_lock_path)
        .map_err(repository_error)?;
    let mut index_lock = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&index_lock_path)
    {
        Ok(file) => file,
        Err(error) => {
            drop(pack_lock);
            std::fs::remove_file(&pack_lock_path).map_err(repository_error)?;
            return Err(repository_error(error));
        }
    };
    let result = (|| {
        pack_lock.write_all(pack).map_err(repository_error)?;
        pack_lock.sync_all().map_err(repository_error)?;
        index_lock.write_all(index).map_err(repository_error)?;
        index_lock.sync_all().map_err(repository_error)?;
        Ok(())
    })();
    drop(pack_lock);
    drop(index_lock);
    if let Err(error) = result {
        std::fs::remove_file(&pack_lock_path).map_err(repository_error)?;
        std::fs::remove_file(&index_lock_path).map_err(repository_error)?;
        return Err(error);
    }
    if pack_path.exists() || index_path.exists() {
        std::fs::remove_file(&pack_lock_path).map_err(repository_error)?;
        std::fs::remove_file(&index_lock_path).map_err(repository_error)?;
        return Err(Error::Conflict(
            "pack appeared while installing verified data".into(),
        ));
    }
    std::fs::rename(&pack_lock_path, pack_path).map_err(repository_error)?;
    std::fs::rename(&index_lock_path, index_path).map_err(repository_error)?;
    Ok(())
}

fn selected_head(advertisement: &Advertisement) -> Result<String, Error> {
    if let Some(head) = advertisement
        .head
        .as_deref()
        .and_then(|name| name.strip_prefix("refs/heads/"))
    {
        return Ok(head.to_owned());
    }
    if let Some(head_oid) = advertisement.head_oid {
        let matches: Vec<_> = advertisement
            .refs
            .iter()
            .filter_map(|(name, id)| {
                (id == &head_oid)
                    .then(|| name.strip_prefix("refs/heads/").map(str::to_owned))
                    .flatten()
            })
            .collect();
        return match matches.as_slice() {
            [branch] => Ok(branch.clone()),
            [] => Err(Error::Unsupported(
                "remote HEAD does not identify an advertised branch".into(),
            )),
            _ => Err(Error::Unsupported(
                "remote HEAD is ambiguous without a symref capability".into(),
            )),
        };
    }
    if advertisement.refs.is_empty() {
        return Ok("main".into());
    }
    Err(Error::Unsupported(
        "remote did not advertise a usable default branch".into(),
    ))
}

fn ref_updates(
    refs: &BTreeMap<String, gix::ObjectId>,
    remote_name: &str,
) -> Vec<(String, gix::ObjectId, bool)> {
    refs.iter()
        .filter_map(|(name, id)| {
            if let Some(branch) = name.strip_prefix("refs/heads/") {
                Some((format!("refs/remotes/{remote_name}/{branch}"), *id, false))
            } else {
                name.strip_prefix("refs/tags/")
                    .map(|_| (name.clone(), *id, true))
            }
        })
        .collect()
}

fn write_remote_config(path: &str, remote_name: &str, url: &str) -> Result<(), Error> {
    use std::io::Write;
    let config = Path::new(path).join("config");
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(config)
        .map_err(repository_error)?;
    writeln!(
        file,
        "\n[remote \"{remote_name}\"]\n\turl = \"{url}\"\n\tfetch = +refs/heads/*:refs/remotes/{remote_name}/*"
    )
    .map_err(repository_error)?;
    file.sync_all().map_err(repository_error)
}

fn read_remote_url(repo: &gix::Repository, remote_name: &str) -> Result<String, Error> {
    validate_remote_name(remote_name)?;
    let remote = repo.find_remote(remote_name).map_err(repository_error)?;
    let url = remote
        .url(gix::remote::Direction::Fetch)
        .ok_or_else(|| Error::InvalidInput("configured remote has no fetch URL".into()))?;
    Ok(url.to_bstring().to_string())
}

fn snapshot_refs(repo: &gix::Repository) -> Result<BTreeMap<String, Option<gix::ObjectId>>, Error> {
    let mut refs = BTreeMap::new();
    let platform = repo.references().map_err(repository_error)?;
    for reference in platform.all().map_err(repository_error)? {
        let reference = reference.map_err(repository_error)?;
        let name = reference
            .name()
            .as_bstr()
            .to_str()
            .map_err(repository_error)?
            .to_owned();
        let id = reference.try_id().map(gix::Id::detach);
        if refs.insert(name, id).is_some() {
            return Err(Error::Repository(
                "duplicate reference while taking snapshot".into(),
            ));
        }
        if refs.len() > MAX_REFERENCES as usize {
            return Err(Error::Unsupported("too many local references".into()));
        }
    }
    Ok(refs)
}

fn install_refs(
    path: &str,
    repo: &gix::Repository,
    refs: &BTreeMap<String, gix::ObjectId>,
    remote_name: &str,
    initial_clone: bool,
    default_branch: &str,
    expected_refs: &BTreeMap<String, Option<gix::ObjectId>>,
) -> Result<Vec<Reference>, Error> {
    let mut installed = Vec::new();
    for (name, id, tag) in ref_updates(refs, remote_name) {
        let previous = expected_refs.get(&name).copied().flatten();
        if tag && expected_refs.contains_key(&name) {
            continue;
        }
        if previous == Some(id) {
            continue;
        }
        update_branch(repo, &name, id, previous)?;
        installed.push((name, id));
    }
    if initial_clone && let Some(id) = refs.get(&format!("refs/heads/{default_branch}")) {
        let name = branch_name(default_branch)?;
        let parent = expected_refs.get(&name).copied().flatten();
        update_branch(repo, &name, *id, parent)?;
    }
    let fresh = open_bare(path)?;
    let mut references = Vec::with_capacity(installed.len());
    for (name, _) in installed {
        let mut reference = fresh
            .find_reference(name.as_str())
            .map_err(repository_error)?;
        references.push(Reference {
            name,
            id: reference
                .peel_to_id()
                .map_err(repository_error)?
                .detach()
                .to_string(),
        });
    }
    references.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(references)
}

pub(crate) fn clone_repository(
    path: &str,
    remote_url: &str,
    remote_name: &str,
    options: &NetworkOptions,
) -> Result<CloneResult, Error> {
    validate_repository_path(path)?;
    validate_remote_name(remote_name)?;
    let url = parse_url(remote_url)?;
    let max_response = validate_options(options, &url)?;
    let advertisement = get_advertisement(&url, options, max_response)?;
    let default_branch = selected_head(&advertisement)?;
    branch_name(&default_branch)?;
    Component::init(path.to_owned(), default_branch.clone())?;
    let repo = open_bare(path)?;
    let expected_refs = snapshot_refs(&repo)?;
    let pack = request_pack(&url, options, max_response, &advertisement, None)?;
    install_pack(&repo, &pack)?;
    write_remote_config(path, remote_name, &url.original)?;
    let references = install_refs(
        path,
        &repo,
        &advertisement.refs,
        remote_name,
        true,
        &default_branch,
        &expected_refs,
    )?;
    Ok(CloneResult {
        default_branch,
        references,
    })
}

pub(crate) fn fetch_repository(
    path: &str,
    remote_name: &str,
    options: &NetworkOptions,
) -> Result<FetchResult, Error> {
    validate_repository_path(path)?;
    validate_remote_name(remote_name)?;
    let repo = open_bare(path)?;
    let expected_refs = snapshot_refs(&repo)?;
    let remote_url = read_remote_url(&repo, remote_name)?;
    let url = parse_url(&remote_url)?;
    let max_response = validate_options(options, &url)?;
    let advertisement = get_advertisement(&url, options, max_response)?;
    let pack = request_pack(&url, options, max_response, &advertisement, Some(&repo))?;
    install_pack(&repo, &pack)?;
    let updated = install_refs(
        path,
        &repo,
        &advertisement.refs,
        remote_name,
        false,
        "",
        &expected_refs,
    )?;
    Ok(FetchResult { updated })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(host: &str) -> NetworkOptions {
        NetworkOptions {
            max_response_bytes: 1024 * 1024,
            max_pack_bytes: 1024 * 1024,
            max_refs: 10,
            allowed_hosts: vec![host.into()],
            credentials: None,
        }
    }

    fn packet(payload: &[u8], output: &mut Vec<u8>) {
        output.extend_from_slice(format!("{:04x}", payload.len() + 4).as_bytes());
        output.extend_from_slice(payload);
    }

    #[test]
    fn url_requires_supported_scheme_safe_authority_and_allowed_host() {
        assert!(parse_url("file:///tmp/repo").is_err());
        assert!(parse_url("http://user@localhost/repo").is_err());
        assert!(parse_url("http://localhost/repo?token=secret").is_err());
        let url = parse_url("http://LOCALHOST:8080/repo.git/").unwrap();
        assert_eq!(url.base_path, "/repo.git");
        assert_eq!(
            validate_options(&options("localhost"), &url).unwrap(),
            1024 * 1024
        );
        assert!(validate_options(&options("example.com"), &url).is_err());
        assert!(validate_options(&options("*"), &url).is_err());
    }

    #[test]
    fn credentials_require_exactly_one_valid_authentication_mode() {
        let credentials = NetworkCredentials {
            username: Some("user".into()),
            password: Some("pass".into()),
            bearer_token: None,
        };
        validate_credentials(&credentials).unwrap();
        assert_eq!(base64(b"user:pass").unwrap(), "dXNlcjpwYXNz");
        let invalid = NetworkCredentials {
            username: Some("user\r\nAuthorization: evil".into()),
            password: Some("pass".into()),
            bearer_token: None,
        };
        assert!(validate_credentials(&invalid).is_err());
    }

    #[test]
    fn packet_parser_rejects_truncation_and_invalid_lengths() {
        assert!(visit_packets(b"0006x", |_| Ok(())).is_err());
        assert!(visit_packets(b"0003", |_| Ok(())).is_err());
        assert!(visit_packets(b"zzzz", |_| Ok(())).is_err());
    }

    #[test]
    fn advertisement_parses_refs_capabilities_and_default_head() {
        let mut data = Vec::new();
        packet(b"# service=git-upload-pack\n", &mut data);
        data.extend_from_slice(b"0000");
        packet(
            b"1111111111111111111111111111111111111111 refs/heads/main\0side-band-64k ofs-delta symref=HEAD:refs/heads/main\n",
            &mut data,
        );
        data.extend_from_slice(b"0000");
        let parsed = parse_advertisement(&data, 10).unwrap();
        assert_eq!(selected_head(&parsed).unwrap(), "main");
        assert!(parsed.side_band_64k && parsed.ofs_delta);
        assert!(parsed.refs.contains_key("refs/heads/main"));
    }

    #[test]
    fn sideband_enforces_pack_prefix_and_budget() {
        let mut data = Vec::new();
        let mut pack = vec![0u8; 32];
        pack.get_mut(..4)
            .expect("the fixture has a four-byte header")
            .copy_from_slice(b"PACK");
        let mut payload = vec![1];
        payload.extend_from_slice(&pack);
        packet(&payload, &mut data);
        data.extend_from_slice(b"0000");
        assert_eq!(unpack_sideband(&data, 32).unwrap(), pack);
        assert!(unpack_sideband(&data, 31).is_err());
    }
}
