#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandLine {
    pub argv: Vec<String>,
    pub stdout: Option<String>,
    pub append: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseError {
    Empty,
    MissingRedirectTarget,
    TooManyArgs,
}

pub fn parse(input: &str) -> Result<CommandLine, ParseError> {
    let mut argv = Vec::new();
    let mut stdout = None;
    let mut append = false;
    let mut iter = input.split_whitespace().peekable();
    while let Some(tok) = iter.next() {
        match tok {
            ">" | ">>" => {
                append = tok == ">>";
                let Some(path) = iter.next() else {
                    return Err(ParseError::MissingRedirectTarget);
                };
                stdout = Some(path.to_string());
            }
            _ => {
                if argv.len() == 32 {
                    return Err(ParseError::TooManyArgs);
                }
                argv.push(tok.to_string());
            }
        }
    }
    if argv.is_empty() {
        return Err(ParseError::Empty);
    }
    Ok(CommandLine {
        argv,
        stdout,
        append,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_command() {
        let parsed = parse("mkdir /tmp/t").unwrap();
        assert_eq!(parsed.argv, ["mkdir", "/tmp/t"]);
        assert_eq!(parsed.stdout, None);
    }

    #[test]
    fn parses_redirect() {
        let parsed = parse("echo hi > /tmp/a").unwrap();
        assert_eq!(parsed.argv, ["echo", "hi"]);
        assert_eq!(parsed.stdout, Some("/tmp/a".into()));
        assert!(!parsed.append);
    }
}
