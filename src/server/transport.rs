use anyhow::{Context, Result, bail};
use serde::Serialize;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Read one MCP stdio message.
///
/// MCP clients use `Content-Length` frames. Older Axon tests and scripts used
/// newline-delimited JSON, so this accepts both on input.
pub async fn read_message<R>(reader: &mut R) -> Result<Option<String>>
where
    R: AsyncBufRead + Unpin,
{
    loop {
        let mut first = String::new();
        if reader.read_line(&mut first).await? == 0 {
            return Ok(None);
        }

        let first = trim_line_end(&first);
        if first.is_empty() {
            continue;
        }

        if is_header_line(first) {
            return read_framed_message(reader, first).await.map(Some);
        }

        return Ok(Some(first.to_string()));
    }
}

pub async fn write_json<W, T>(writer: &mut W, value: &T) -> Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let message = serde_json::to_string(value)?;
    write_message(writer, &message).await
}

pub async fn write_message<W>(writer: &mut W, message: &str) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let header = format!("Content-Length: {}\r\n\r\n", message.len());
    writer.write_all(header.as_bytes()).await?;
    writer.write_all(message.as_bytes()).await?;
    writer.flush().await?;
    Ok(())
}

async fn read_framed_message<R>(reader: &mut R, first: &str) -> Result<String>
where
    R: AsyncBufRead + Unpin,
{
    let mut content_length = parse_content_length(first)?;

    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await? == 0 {
            bail!("MCP frame ended before the header block was complete");
        }

        let line = trim_line_end(&line);
        if line.is_empty() {
            break;
        }

        if let Some(length) = parse_content_length(line)? {
            content_length = Some(length);
        }
    }

    let length = content_length.context("MCP frame missing Content-Length header")?;
    let mut body = vec![0; length];
    reader.read_exact(&mut body).await?;
    String::from_utf8(body).context("MCP frame body was not valid UTF-8")
}

fn trim_line_end(line: &str) -> &str {
    line.trim_end_matches(['\r', '\n'])
}

fn is_header_line(line: &str) -> bool {
    let Some((name, _)) = line.split_once(':') else {
        return false;
    };

    !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn parse_content_length(line: &str) -> Result<Option<usize>> {
    let Some((name, value)) = line.split_once(':') else {
        return Ok(None);
    };

    if !name.eq_ignore_ascii_case("content-length") {
        return Ok(None);
    }

    let length = value
        .trim()
        .parse::<usize>()
        .with_context(|| format!("invalid Content-Length header: {line}"))?;
    Ok(Some(length))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::BufReader;

    #[tokio::test]
    async fn reads_content_length_frame() {
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#;
        let input = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        let mut reader = BufReader::new(input.as_bytes());

        let message = read_message(&mut reader).await.unwrap().expect("message");

        assert_eq!(message, body);
    }

    #[tokio::test]
    async fn reads_legacy_newline_json() {
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#;
        let input = format!("{body}\n");
        let mut reader = BufReader::new(input.as_bytes());

        let message = read_message(&mut reader).await.unwrap().expect("message");

        assert_eq!(message, body);
    }

    #[tokio::test]
    async fn writes_content_length_frame() {
        let mut output = Vec::new();

        write_message(&mut output, "{}").await.unwrap();

        assert_eq!(
            String::from_utf8(output).unwrap(),
            "Content-Length: 2\r\n\r\n{}"
        );
    }
}
