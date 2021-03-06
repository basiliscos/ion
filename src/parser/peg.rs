use std::io::{stderr, Write};
use std::fmt;

use shell::flow_control::{ElseIf, Statement, FunctionArgument, Type};
use self::grammar::parse_;
use shell::directory_stack::DirectoryStack;
use shell::{JobKind, Job};
use shell::variables::Variables;
use super::{expand_string, ExpanderFunctions, Select};
use super::assignments::parse_assignment;
use super::pipelines;

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum RedirectFrom { Stdout, Stderr, Both}

#[derive(Debug, PartialEq, Clone)]
pub struct Redirection {
    pub from:   RedirectFrom,
    pub file:   String,
    pub append: bool
}

/// Represents input that a process could initially receive from `stdin`
#[derive(Debug, PartialEq, Clone)]
pub enum Input {
    /// A file; the contents of said file will be written to the `stdin` of a process
    File(String),
    /// A string literal that is written to the `stdin` of a process.
    /// If there is a second string, that second string is the EOF phrase for the heredoc.
    HereString(String),
}

#[derive(Debug, PartialEq, Clone)]
pub struct Pipeline {
    pub jobs:   Vec<Job>,
    pub stdout: Option<Redirection>,
    pub stdin:  Option<Input>,
}

impl Pipeline {
    pub fn new(jobs: Vec<Job>, stdin: Option<Input>, stdout: Option<Redirection>) -> Self {
        Pipeline { jobs, stdin, stdout }
    }

    pub fn expand(&mut self, variables: &Variables, dir_stack: &DirectoryStack) {
        let expanders = get_expanders!(variables, dir_stack);
        for job in &mut self.jobs {
            job.expand(&expanders);
        }

        let stdin = match self.stdin {
            Some(Input::File(ref s)) => {
                Some(Input::File(expand_string(s, &expanders, false).join(" ")))
            },
            Some(Input::HereString(ref s)) => {
                Some(Input::HereString(expand_string(s, &expanders, true).join(" ")))
            },
            None => None
        };

        self.stdin = stdin;

        if let Some(stdout) = self.stdout.iter_mut().next() {
            stdout.file = expand_string(stdout.file.as_str(), &expanders, false).join(" ");
        }
    }
}

impl fmt::Display for Pipeline {

    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut tokens: Vec<String> = Vec::with_capacity(self.jobs.len());
        for job in &self.jobs {
            tokens.extend(job.args.clone().into_iter());
            match job.kind {
                JobKind::Last => (),
                JobKind::And => tokens.push("&&".into()),
                JobKind::Or => tokens.push("||".into()),
                JobKind::Background => tokens.push("&".into()),
                JobKind::Pipe(RedirectFrom::Stdout) => tokens.push("|".into()),
                JobKind::Pipe(RedirectFrom::Stderr) => tokens.push("^|".into()),
                JobKind::Pipe(RedirectFrom::Both) => tokens.push("&|".into()),
            }
        }
        match self.stdin {
            None => (),
            Some(Input::File(ref file)) => {
                tokens.push("<".into());
                tokens.push(file.clone());
            },
            Some(Input::HereString(ref string)) => {
                tokens.push("<<<".into());
                tokens.push(string.clone());
            },
        }
        if let Some(ref outfile) = self.stdout {
            match outfile.from {
                RedirectFrom::Stdout => {
                    tokens.push((if outfile.append { ">>" } else { ">" }).into());
                },
                RedirectFrom::Stderr => {
                    tokens.push((if outfile.append { "^>>" } else { "^>" }).into());
                },
                RedirectFrom::Both => {
                    tokens.push((if outfile.append { "&>>" } else { "&>" }).into());
                }
            }
            tokens.push(outfile.file.clone());
        }

        write!(f, "{}", tokens.join(" "))
    }

}

pub fn parse(code: &str) -> Statement {
    let mut trimmed = code.trim();
    match trimmed {
        "end"      => return Statement::End,
        "break"    => return Statement::Break,
        "continue" => return Statement::Continue,
        _ if trimmed.starts_with("let ") => {
            trimmed = trimmed[4..].trim_left();
            return Statement::Let { expression: parse_assignment(trimmed) }
        },
        _ if trimmed.starts_with("export ") => {
            trimmed = trimmed[7..].trim_left();
            return Statement::Export(parse_assignment(trimmed))
        },
        _ if trimmed.starts_with("if ") => {
            trimmed = trimmed[3..].trim_left();

            let result = pipelines::Collector::run(trimmed).map(|p| {
                Statement::If {
                    expression: p,
                    success: Vec::new(),
                    else_if: Vec::new(),
                    failure: Vec::new()
                    }
                });

            match result {
                Ok(statement) => return statement,
                Err(err) => {
                    let stderr = stderr();
                    let _ = writeln!(stderr.lock(), "ion: Syntax {}", err);
                    return Statement::Default
                }
            }
        }
        "else" => return Statement::Else,
        _ if trimmed.starts_with("else") => {
            let mut trimmed = trimmed[4..].trim_left();
            if trimmed.len() == 0 {
                return Statement::Else
            } else if trimmed.starts_with("if ") {
                trimmed = trimmed[3..].trim_left();
                let result = pipelines::Collector::run(&trimmed).map(|p| {
                    Statement::ElseIf(ElseIf {
                        expression: p,
                        success:    Vec::new(),
                    })
                });

                match result {
                    Ok(statement) => return statement,
                    Err(err) => {
                        let stderr = stderr();
                        let _ = writeln!(stderr.lock(), "ion: Syntax {}", err);
                        return Statement::Default
                    }
                }
            }
        }
        _ => ()
    }

    match parse_(trimmed) {
        Ok(code_ok) => code_ok,
        Err(err) => {
            let stderr = stderr();
            let _ = writeln!(stderr.lock(), "ion: Syntax {}", err);
            Statement::Default
        }
    }
}

pub fn get_function_args(args: Vec<String>) -> Option<Vec<FunctionArgument>> {
    let mut fn_args = Vec::with_capacity(args.len());
    for argument in args.into_iter() {
        let length = argument.len();
        let argument = if argument.ends_with(":int") {
            if length <= 4 { return None }
            let arg = &argument[..length-4];
            if arg.contains(':') { return None }
            FunctionArgument::Typed (
                arg.to_owned(),
                Type::Int
            )
        } else if argument.ends_with(":float") {
            if length <= 6 { return None }
            let arg = &argument[..length-6];
            if arg.contains(':') { return None }
            FunctionArgument::Typed (
                arg.to_owned(),
                Type::Float
            )
        } else if argument.ends_with(":bool") {
            if length <= 5 { return None }
            let arg = &argument[..length-5];
            if arg.contains(':') { return None }
            FunctionArgument::Typed (
                arg.to_owned(),
                Type::Bool
            )
        } else {
            FunctionArgument::Untyped(argument)
        };
        fn_args.push(argument);
    }

    Some(fn_args)
}

mod grammar {
    include!(concat!(env!("OUT_DIR"), "/grammar.rs"));
}

#[cfg(test)]
mod tests {
    use super::grammar::*;
    use super::*;
    use shell::flow_control::Statement;
    use shell::JobKind;

    #[test]
    fn full_script() {
        pipelines(r#"if a == a
  echo true a == a

  if b != b
    echo true b != b
  else
    echo false b != b

    if 3 > 2
      echo true 3 > 2
    else
      echo false 3 > 2
    fi
  fi
else
  echo false a == a
fi
"#)
            .unwrap();  // Make sure it parses
    }

    #[test]
    fn leading_and_trailing_junk() {
        pipelines(r#"

# comment
   # comment


    if a == a
  echo true a == a  # Line ending commment

  if b != b
    echo true b != b
  else
    echo false b != b

    if 3 > 2
      echo true 3 > 2
    else
      echo false 3 > 2
    fi
  fi
else
  echo false a == a
      fi

# comment

"#).unwrap();  // Make sure it parses
    }
    #[test]
    fn parsing_ifs() {
        // Default case where spaced normally
        let parsed_if = parse("if test 1 -eq 2");
        let correct_parse = Statement::If {
            expression: Pipeline::new(
                vec!(Job::new(
                    vec![
                        "test".to_owned(),
                        "1".to_owned(),
                        "-eq".to_owned(),
                        "2".to_owned(),
                    ].into_iter().collect(), JobKind::Last)
                ), None, None),
            success: vec!(),
            else_if: vec!(),
            failure: vec!()
        };
        assert_eq!(correct_parse, parsed_if);

        // Trailing spaces after final value
        let parsed_if = parse("if test 1 -eq 2         ");
        assert_eq!(correct_parse, parsed_if);
    }

    #[test]
    fn parsing_elses() {
        // Default case where spaced normally
        let mut parsed_if = parse("else");
        let correct_parse = Statement::Else;
        assert_eq!(correct_parse, parsed_if);

        // Trailing spaces after final value
        parsed_if = parse("else         ");
        assert_eq!(correct_parse, parsed_if);

        // Leading spaces after final value
        parsed_if = parse("         else");
        assert_eq!(correct_parse, parsed_if);
    }

    #[test]
    fn parsing_ends() {
        // Default case where spaced normally
        let parsed_if = parse("end");
        let correct_parse = Statement::End;
        assert_eq!(correct_parse, parsed_if);

        // Trailing spaces after final value
        let parsed_if = parse("end         ");
        let correct_parse = Statement::End;
        assert_eq!(correct_parse, parsed_if);

        // Leading spaces after final value
        let parsed_if = parse("         end");
        let correct_parse = Statement::End;
        assert_eq!(correct_parse, parsed_if);
    }

    #[test]
    fn parsing_functions() {
        // Default case where spaced normally
        let parsed_if = parse("fn bob");
        let correct_parse = Statement::Function{
            description: "".into(),
            name:        "bob".into(),
            args:        Default::default(),
            statements:  Default::default(),
        };
        assert_eq!(correct_parse, parsed_if);

        // Trailing spaces after final value
        let parsed_if = parse("fn bob        ");
        assert_eq!(correct_parse, parsed_if);

        // Leading spaces after final value
        let parsed_if = parse("         fn bob");

        // Default case where spaced normally
        let parsed_if = parse("fn bob a b");
        let correct_parse = Statement::Function{
            description: "".into(),
            name:        "bob".into(),
            args:        vec![FunctionArgument::Untyped("a".to_owned()), FunctionArgument::Untyped("b".to_owned())],
            statements:  Default::default(),
        };
        assert_eq!(correct_parse, parsed_if);

        // Trailing spaces after final value
        let parsed_if = parse("fn bob a b       ");
        assert_eq!(correct_parse, parsed_if);

        let parsed_if = parse("fn bob a b --bob is a nice function");
        let correct_parse = Statement::Function{
            description: "bob is a nice function".to_string(),
            name:        "bob".into(),
            args:        vec!(FunctionArgument::Untyped("a".to_owned()), FunctionArgument::Untyped("b".to_owned())),
            statements:  vec!()
        };
        assert_eq!(correct_parse, parsed_if);
        let parsed_if = parse("fn bob a b --          bob is a nice function");
        assert_eq!(correct_parse, parsed_if);
        let parsed_if = parse("fn bob a b      --bob is a nice function");
        assert_eq!(correct_parse, parsed_if);
    }
}
