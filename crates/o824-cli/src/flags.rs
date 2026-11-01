//! A minimal flag parser for the kernel's subcommands.
//!
//! It follows the reference implementation's use of Go's `flag` package for the
//! parts that are observable: `--flag`, `--flag=value`, `--flag value`, and the
//! error a mistyped flag produces.

use o824_errs::{Code, Error};

/// One accepted flag: its name (without dashes) and whether it takes a value.
#[derive(Clone, Copy, Debug)]
pub struct FlagSpec {
    /// The flag name without leading dashes.
    pub name: &'static str,
    /// Whether the flag takes a value.
    pub takes_value: bool,
}

impl FlagSpec {
    /// A boolean flag.
    pub const fn boolean(name: &'static str) -> FlagSpec {
        FlagSpec {
            name,
            takes_value: false,
        }
    }

    /// A flag that takes a value.
    pub const fn valued(name: &'static str) -> FlagSpec {
        FlagSpec {
            name,
            takes_value: true,
        }
    }
}

/// The parsed flags of one invocation.
#[derive(Debug, Default)]
pub struct FlagSet {
    specs: Vec<FlagSpec>,
    values: Vec<(&'static str, Option<String>)>,
    positional: Vec<String>,
}

impl FlagSet {
    /// Creates a set that accepts `specs`.
    pub fn new(specs: Vec<FlagSpec>) -> FlagSet {
        FlagSet {
            specs,
            values: Vec::new(),
            positional: Vec::new(),
        }
    }

    /// Parses `args`.
    ///
    /// `command` only names the failing invocation in the usage error.
    pub fn parse(&mut self, command: &str, args: &[String]) -> Result<(), Error> {
        let mut index = 0;
        while index < args.len() {
            let argument = &args[index];
            index += 1;

            if argument == "--" {
                self.positional.extend(args[index..].iter().cloned());
                return Ok(());
            }
            let Some(name) = argument
                .strip_prefix("--")
                .or_else(|| argument.strip_prefix('-'))
            else {
                self.positional.push(argument.clone());
                continue;
            };
            if name.is_empty() {
                self.positional.push(argument.clone());
                continue;
            }

            let (name, inline_value) = match name.split_once('=') {
                Some((name, value)) => (name, Some(value.to_owned())),
                None => (name, None),
            };
            let Some(spec) = self.specs.iter().find(|spec| spec.name == name).copied() else {
                return Err(Error::new(
                    Code::Usage,
                    format!("o824 {command}: flag provided but not defined: -{name}"),
                ));
            };

            if spec.takes_value {
                let value = match inline_value {
                    Some(value) => value,
                    None => match args.get(index) {
                        Some(value) => {
                            index += 1;
                            value.clone()
                        }
                        None => {
                            return Err(Error::new(
                                Code::Usage,
                                format!("o824 {command}: flag needs an argument: -{name}"),
                            ));
                        }
                    },
                };
                self.values.push((spec.name, Some(value)));
            } else {
                self.values.push((spec.name, inline_value));
            }
        }
        Ok(())
    }

    /// True when a boolean flag was passed.
    pub fn flag(&self, name: &str) -> bool {
        self.values.iter().any(|(flag, _)| *flag == name)
    }

    /// The last value passed for a valued flag.
    pub fn value(&self, name: &str) -> Option<&str> {
        self.values
            .iter()
            .rev()
            .find(|(flag, _)| *flag == name)
            .and_then(|(_, value)| value.as_deref())
    }

    /// The arguments that were not flags.
    pub fn positional(&self) -> &[String] {
        &self.positional
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn specs() -> Vec<FlagSpec> {
        vec![
            FlagSpec::valued("root"),
            FlagSpec::valued("home"),
            FlagSpec::boolean("json"),
        ]
    }

    fn parse(args: &[&str]) -> Result<FlagSet, Error> {
        let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
        let mut set = FlagSet::new(specs());
        set.parse("doctor", &args)?;
        Ok(set)
    }

    #[test]
    fn both_flag_spellings_are_accepted() {
        let set = parse(&["--root", "C:\\x", "--home=C:\\y", "--json"]).expect("parse");
        assert_eq!(set.value("root"), Some("C:\\x"));
        assert_eq!(set.value("home"), Some("C:\\y"));
        assert!(set.flag("json"));
    }

    #[test]
    fn a_mistyped_flag_is_a_usage_error() {
        let error = parse(&["--nope"]).expect_err("must fail");
        assert_eq!(error.code, Code::Usage);
        assert!(error
            .fallback
            .contains("flag provided but not defined: -nope"));
    }

    #[test]
    fn a_valued_flag_without_a_value_is_a_usage_error() {
        let error = parse(&["--root"]).expect_err("must fail");
        assert_eq!(error.code, Code::Usage);
        assert!(error.fallback.contains("flag needs an argument: -root"));
    }

    #[test]
    fn positional_arguments_are_kept() {
        let set = parse(&["--json", "extra"]).expect("parse");
        assert_eq!(set.positional(), ["extra".to_owned()]);
    }
}
