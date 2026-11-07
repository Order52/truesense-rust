//! A small re-implementation of the parts of Python's `argparse` that the TrueSense command line
//! used, so that usage lines, `-h` pages and error messages are the same text as before.
//!
//! Scripts and people have learned the exact words of the Python edition ("argument mode: invalid
//! choice: ...", the way a long usage line wraps), and the shell completions are generated from the
//! same tree. Everything here is data driven: [`Command`] describes a parser, [`parse`](parse::parse)
//! walks it the way `argparse` does (including its abbreviation of long options, its handling of
//! negative numbers and of optional positionals), and [`help`] formats it the way
//! `HelpFormatter` does. The behaviour follows Python 3.14, the version the reference output was
//! captured with.
#![forbid(unsafe_code)]

pub(super) mod help;
pub(super) mod parse;
pub(super) mod pyfmt;
pub(super) mod textwrap;

use std::collections::BTreeMap;

/// How a help text is shown.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) enum Help {
    /// No help given: the argument is listed without text (a subcommand is not listed at all).
    #[default]
    None,
    /// The text, whitespace-normalised and re-wrapped when shown.
    Text(String),
    /// `argparse.SUPPRESS`: left out of usage and help. (For a subcommand the Python edition showed
    /// the marker `==SUPPRESS==`: kept, because the reference output has it.)
    Suppress,
}

/// How many words a positional takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Nargs {
    /// Exactly one.
    One,
    /// Zero or one (`?`).
    Optional,
    /// Zero or more (`*`).
    ZeroOrMore,
    /// One or more (`+`).
    OneOrMore,
}

/// What an option does with its words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Action {
    /// Keeps the last value (also a positional).
    Store,
    /// `store_true`: a switch.
    StoreTrue,
    /// `append`: collects every use into a list.
    Append,
    /// `-h`: prints the help and ends the program.
    Help,
    /// `-V`: prints this text and ends the program.
    Version(String),
}

/// The conversion of a word before it is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Ty {
    /// Kept as text.
    Str,
    /// A whole number (`int(text)`).
    Int,
    /// A decimal number (`float(text)`).
    Float,
}

/// A parsed value.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Val {
    /// Not given and no default.
    None,
    /// A switch.
    Bool(bool),
    /// A whole number.
    Int(i64),
    /// A decimal number.
    Float(f64),
    /// Text.
    Str(String),
    /// Several words.
    List(Vec<String>),
}

/// One option or positional argument.
#[derive(Debug, Clone)]
pub(super) struct Arg {
    /// The key the value is stored under.
    pub(super) dest: String,
    /// The option strings (`--color`); empty for a positional.
    pub(super) flags: Vec<String>,
    /// What it does.
    pub(super) action: Action,
    /// How many words a positional takes.
    pub(super) nargs: Nargs,
    /// The conversion.
    pub(super) ty: Ty,
    /// The allowed values as written in help; empty means any.
    pub(super) choices: Vec<String>,
    /// The value when absent.
    pub(super) default: Val,
    /// The help text.
    pub(super) help: Help,
    /// The name shown for the value.
    pub(super) metavar: Option<String>,
}

impl Arg {
    fn base(dest: &str, flags: Vec<String>, action: Action, default: Val) -> Self {
        Self {
            dest: dest.to_owned(),
            flags,
            action,
            nargs: Nargs::One,
            ty: Ty::Str,
            choices: Vec::new(),
            default,
            help: Help::None,
            metavar: None,
        }
    }

    /// A positional word.
    pub(super) fn pos(dest: &str) -> Self {
        Self::base(dest, Vec::new(), Action::Store, Val::None)
    }

    /// An option that takes a value; the key is the flag without dashes, `-` as `_`.
    pub(super) fn opt(flag: &str) -> Self {
        let dest = flag.trim_start_matches('-').replace('-', "_");
        Self::base(&dest, vec![flag.to_owned()], Action::Store, Val::None)
    }

    /// A switch (`store_true`).
    pub(super) fn flag(flag: &str) -> Self {
        let dest = flag.trim_start_matches('-').replace('-', "_");
        Self::base(&dest, vec![flag.to_owned()], Action::StoreTrue, Val::Bool(false))
    }

    /// An option that may be given several times (`append`); the default is an empty list.
    pub(super) fn append(flag: &str) -> Self {
        let dest = flag.trim_start_matches('-').replace('-', "_");
        Self::base(&dest, vec![flag.to_owned()], Action::Append, Val::List(Vec::new()))
    }

    /// Stores under another key (`dest=`).
    pub(super) fn dest(mut self, dest: &str) -> Self {
        self.dest = dest.to_owned();
        self
    }

    /// The allowed words.
    pub(super) fn choices(mut self, choices: &[&str]) -> Self {
        self.choices = choices.iter().map(|c| (*c).to_owned()).collect();
        self
    }

    /// The allowed words, owned.
    pub(super) fn choices_owned(mut self, choices: Vec<String>) -> Self {
        self.choices = choices;
        self
    }

    /// `nargs='?'`.
    pub(super) fn optional(mut self) -> Self {
        self.nargs = Nargs::Optional;
        self
    }

    /// `nargs='*'`.
    pub(super) fn star(mut self) -> Self {
        self.nargs = Nargs::ZeroOrMore;
        self
    }

    /// `nargs='+'`.
    pub(super) fn plus(mut self) -> Self {
        self.nargs = Nargs::OneOrMore;
        self
    }

    /// `type=int`.
    pub(super) fn int(mut self) -> Self {
        self.ty = Ty::Int;
        self
    }

    /// `type=float`.
    pub(super) fn float(mut self) -> Self {
        self.ty = Ty::Float;
        self
    }

    /// A text default.
    pub(super) fn default_str(mut self, text: &str) -> Self {
        self.default = Val::Str(text.to_owned());
        self
    }

    /// A whole-number default.
    pub(super) fn default_int(mut self, n: i64) -> Self {
        self.default = Val::Int(n);
        self
    }

    /// A decimal default.
    pub(super) fn default_float(mut self, n: f64) -> Self {
        self.default = Val::Float(n);
        self
    }

    /// The help text.
    pub(super) fn help(mut self, text: &str) -> Self {
        self.help = Help::Text(text.to_owned());
        self
    }

    /// `help=argparse.SUPPRESS`.
    pub(super) fn hidden(mut self) -> Self {
        self.help = Help::Suppress;
        self
    }

    /// The name shown for the value.
    pub(super) fn metavar(mut self, name: &str) -> Self {
        self.metavar = Some(name.to_owned());
        self
    }

    /// Whether this is a positional argument.
    pub(super) fn is_positional(&self) -> bool {
        self.flags.is_empty()
    }

    /// Whether the option takes no word.
    pub(super) fn takes_nothing(&self) -> bool {
        matches!(self.action, Action::StoreTrue | Action::Help | Action::Version(_))
    }

    /// Whether a positional must be given.
    pub(super) fn required(&self) -> bool {
        self.is_positional() && matches!(self.nargs, Nargs::One | Nargs::OneOrMore)
    }
}

/// The subcommands of a parser.
#[derive(Debug, Clone)]
pub(super) struct Subcommands {
    /// The key the chosen name is stored under.
    pub(super) dest: String,
    /// The name shown instead of the list of choices.
    pub(super) metavar: Option<String>,
    /// The subcommands in the order they were added.
    pub(super) commands: Vec<Command>,
}

/// One parser: the program, a subcommand or the verb of an area.
#[derive(Debug, Clone)]
pub(super) struct Command {
    /// The name that selects it.
    pub(super) name: String,
    /// The line in the parent's listing; [`Help::None`] leaves it out.
    pub(super) help: Help,
    /// Printed raw under the usage line (the program only).
    pub(super) description: Option<String>,
    /// Printed raw at the end of the help (the program only).
    pub(super) epilog: Option<String>,
    /// The arguments, not counting the implicit `-h`.
    pub(super) args: Vec<Arg>,
    /// The subcommands.
    pub(super) subs: Option<Subcommands>,
}

impl Command {
    /// A parser with this name.
    pub(super) fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            help: Help::None,
            description: None,
            epilog: None,
            args: Vec::new(),
            subs: None,
        }
    }

    /// The line shown in the parent's listing.
    pub(super) fn help(mut self, text: &str) -> Self {
        self.help = Help::Text(text.to_owned());
        self
    }

    /// Listed with the `==SUPPRESS==` marker, as the Python edition did for `auto-run`.
    pub(super) fn help_suppressed(mut self) -> Self {
        self.help = Help::Suppress;
        self
    }

    /// Adds an argument.
    pub(super) fn arg(mut self, arg: Arg) -> Self {
        self.args.push(arg);
        self
    }

    /// Adds the subcommands, stored under `dest`.
    pub(super) fn subs(mut self, dest: &str, commands: Vec<Command>) -> Self {
        self.subs = Some(Subcommands { dest: dest.to_owned(), metavar: None, commands });
        self
    }

    /// Names the list of subcommands instead of showing the braces.
    pub(super) fn subs_metavar(mut self, name: &str) -> Self {
        if let Some(s) = self.subs.as_mut() {
            s.metavar = Some(name.to_owned());
        }
        self
    }

    /// The implicit `-h` followed by the declared arguments, in the order `argparse` held them
    /// (the root's `--json` and `-V` come after `-h` too).
    pub(super) fn all_args(&self) -> Vec<Arg> {
        let mut help = Arg::flag("--help").dest("help").help("show this help message and exit");
        help.flags = vec!["-h".to_owned(), "--help".to_owned()];
        help.action = Action::Help;
        let mut all = vec![help];
        all.extend(self.args.iter().cloned());
        all
    }
}

/// What a parse leaves behind: the values by key and the chain of subcommands that was chosen.
#[derive(Debug, Clone, Default)]
pub(super) struct Matches {
    values: BTreeMap<String, Val>,
    /// The subcommand names in the order they were typed (`["power", "set"]`).
    pub(super) path: Vec<String>,
}

impl Matches {
    pub(super) fn set(&mut self, dest: &str, value: Val) {
        self.values.insert(dest.to_owned(), value);
    }

    pub(super) fn get(&self, dest: &str) -> &Val {
        self.values.get(dest).unwrap_or(&Val::None)
    }

    /// A text value.
    pub(super) fn str(&self, dest: &str) -> Option<&str> {
        match self.get(dest) {
            Val::Str(s) => Some(s),
            _ => None,
        }
    }

    /// A switch.
    pub(super) fn flag(&self, dest: &str) -> bool {
        matches!(self.get(dest), Val::Bool(true))
    }

    /// A whole number.
    pub(super) fn int(&self, dest: &str) -> Option<i64> {
        match self.get(dest) {
            Val::Int(n) => Some(*n),
            _ => None,
        }
    }

    /// A decimal number (a whole number counts).
    pub(super) fn float(&self, dest: &str) -> Option<f64> {
        match self.get(dest) {
            Val::Float(n) => Some(*n),
            Val::Int(n) => Some(*n as f64),
            _ => None,
        }
    }

    /// Several words.
    pub(super) fn list(&self, dest: &str) -> Vec<String> {
        match self.get(dest) {
            Val::List(l) => l.clone(),
            _ => Vec::new(),
        }
    }
}
