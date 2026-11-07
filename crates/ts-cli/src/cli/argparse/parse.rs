//! The argument parser: a port of `argparse.ArgumentParser._parse_known_args` for the features the
//! command line uses (options with one word or none, positionals with `?`, `*` and `+`,
//! subcommands, abbreviated long options, negative numbers as words).
//!
//! Each word of the command line is first classified as an option string (`O`) or a plain word
//! (`A`); positionals are then matched against the pattern of what follows, exactly as `argparse`
//! does with a regular expression. That is what makes `fan curve --cpu 1:0 save x` work (the
//! optional positionals are not used up before the option) and what produces the same error text.
#![forbid(unsafe_code)]

use std::io::Write;

use super::help::{format_help, format_usage};
use super::pyfmt::{parse_float, parse_int, repr_str};
use super::{Action, Arg, Command, Matches, Nargs, Ty, Val};

/// Why the parse stopped.
#[derive(Debug)]
pub(in crate::cli) enum Stop {
    /// Help or the version was printed, or an error was reported: end with this exit code.
    Exit(i32),
}

/// Where the parser writes.
pub(in crate::cli) struct Sinks<'a> {
    /// Standard output (help, version).
    pub(in crate::cli) out: &'a mut dyn Write,
    /// Standard error (usage and the error message).
    pub(in crate::cli) err: &'a mut dyn Write,
    /// The terminal width the help is laid out for.
    pub(in crate::cli) columns: usize,
}

/// Parses `args` for the program `root`. Returns the matches, or the exit code when the parser
/// already said what there was to say.
pub(in crate::cli) fn parse(
    root: &Command,
    prog: &str,
    args: &[String],
    sinks: &mut Sinks<'_>,
) -> Result<Matches, Stop> {
    let mut matches = Matches::default();
    let mut extras = Vec::new();
    parse_known(root, prog, args, &mut matches, &mut extras, sinks)?;
    if !extras.is_empty() {
        let message = format!("unrecognized arguments: {}", extras.join(" "));
        return Err(error(root, prog, &message, sinks));
    }
    Ok(matches)
}

/// Prints the usage and the message to stderr; the exit code of a usage error is 2.
fn error(cmd: &Command, prog: &str, message: &str, sinks: &mut Sinks<'_>) -> Stop {
    let usage = format_usage(cmd, prog, sinks.columns);
    let _ = writeln!(sinks.err, "{usage}{prog}: error: {message}");
    Stop::Exit(2)
}

/// How the arguments of a parser are found by option string.
struct Options<'a> {
    args: &'a [Arg],
}

impl Options<'_> {
    fn exact(&self, text: &str) -> Option<usize> {
        self.args.iter().position(|a| a.flags.iter().any(|f| f == text))
    }

    /// Every option string, with its argument index.
    fn all_flags(&self) -> impl Iterator<Item = (usize, &str)> {
        self.args.iter().enumerate().flat_map(|(i, a)| a.flags.iter().map(move |f| (i, f.as_str())))
    }
}

/// An option string found on the command line.
#[derive(Debug, Clone)]
struct Found {
    /// The argument it belongs to; `None` for an unknown option.
    arg: Option<usize>,
    /// The option string as matched (the full flag, not the abbreviation).
    flag: String,
    /// The word glued to it with `=` (or the rest of a short option).
    explicit: Option<String>,
}

fn is_negative_number(text: &str) -> bool {
    let Some(rest) = text.strip_prefix('-') else { return false };
    if rest.is_empty() {
        return false;
    }
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    match rest.split_once('.') {
        None => digits(rest),
        Some((whole, frac)) => (whole.is_empty() || digits(whole)) && digits(frac),
    }
}

/// `_parse_optional`: `Ok(None)` is a plain word, `Ok(Some)` an option string.
fn classify(text: &str, options: &Options<'_>) -> Result<Option<Found>, String> {
    if !text.starts_with('-') {
        return Ok(None);
    }
    if let Some(i) = options.exact(text) {
        return Ok(Some(Found { arg: Some(i), flag: text.to_owned(), explicit: None }));
    }
    if text.len() == 1 {
        return Ok(None);
    }
    if let Some((flag, explicit)) = text.split_once('=') {
        if let Some(i) = options.exact(flag) {
            return Ok(Some(Found {
                arg: Some(i),
                flag: flag.to_owned(),
                explicit: Some(explicit.to_owned()),
            }));
        }
    }
    let tuples = option_tuples(text, options);
    match tuples.len() {
        0 => {}
        1 => return Ok(tuples.into_iter().next()),
        _ => {
            let names: Vec<&str> = tuples.iter().map(|t| t.flag.as_str()).collect();
            return Err(format!("ambiguous option: {text} could match {}", names.join(", ")));
        }
    }
    // The parser has no option that looks like a negative number, so such a word is a value.
    if is_negative_number(text) || text.contains(' ') {
        return Ok(None);
    }
    Ok(Some(Found { arg: None, flag: text.to_owned(), explicit: None }))
}

/// `_get_option_tuples`: the options an abbreviation could mean.
fn option_tuples(text: &str, options: &Options<'_>) -> Vec<Found> {
    let mut found = Vec::new();
    if text.starts_with("--") {
        let (prefix, explicit) = match text.split_once('=') {
            Some((p, e)) => (p, Some(e.to_owned())),
            None => (text, None),
        };
        for (i, flag) in options.all_flags() {
            if flag.starts_with(prefix) {
                found.push(Found { arg: Some(i), flag: flag.to_owned(), explicit: explicit.clone() });
            }
        }
    } else {
        let short: String = text.chars().take(2).collect();
        let rest: String = text.chars().skip(2).collect();
        for (i, flag) in options.all_flags() {
            if flag == short {
                found.push(Found { arg: Some(i), flag: flag.to_owned(), explicit: Some(rest.clone()) });
            } else if flag.starts_with(text) {
                found.push(Found { arg: Some(i), flag: flag.to_owned(), explicit: None });
            }
        }
    }
    found
}

/// What a positional's regular expression allows (`(-*A-*)` and friends).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    One,
    Optional,
    ZeroOrMore,
    OneOrMore,
    Subcommand,
}

fn count_run(s: &[u8], from: usize, allowed: &[u8]) -> usize {
    s[from.min(s.len())..].iter().take_while(|c| allowed.contains(c)).count()
}

/// The end positions a slot can take from `at`, most greedy first, like the regex engine.
fn candidates(slot: Slot, s: &[u8], at: usize) -> Vec<usize> {
    let mut ends = Vec::new();
    let leading = count_run(s, at, b"-");
    for lead in (0..=leading).rev() {
        let core_at = at + lead;
        let mut cores: Vec<usize> = Vec::new();
        match slot {
            Slot::One => {
                if s.get(core_at) == Some(&b'A') {
                    cores.push(core_at + 1);
                }
            }
            Slot::Optional => {
                if s.get(core_at) == Some(&b'A') {
                    cores.push(core_at + 1);
                }
                cores.push(core_at);
            }
            Slot::ZeroOrMore => {
                let run = count_run(s, core_at, b"A-");
                cores.extend((0..=run).rev().map(|k| core_at + k));
            }
            Slot::OneOrMore => {
                if s.get(core_at) == Some(&b'A') {
                    let run = count_run(s, core_at + 1, b"A-");
                    cores.extend((0..=run).rev().map(|k| core_at + 1 + k));
                }
            }
            Slot::Subcommand => {
                if s.get(core_at) == Some(&b'A') {
                    let run = count_run(s, core_at + 1, b"-AO");
                    cores.extend((0..=run).rev().map(|k| core_at + 1 + k));
                }
            }
        }
        for core in cores {
            if slot == Slot::ZeroOrMore || slot == Slot::OneOrMore || slot == Slot::Subcommand {
                ends.push(core);
            } else {
                let trailing = count_run(s, core, b"-");
                ends.extend((0..=trailing).rev().map(|k| core + k));
            }
        }
    }
    ends
}

fn match_slots(slots: &[Slot], s: &[u8], at: usize, lens: &mut Vec<usize>) -> Option<usize> {
    let Some((first, rest)) = slots.split_first() else { return Some(at) };
    for end in candidates(*first, s, at) {
        lens.push(end - at);
        if let Some(finish) = match_slots(rest, s, end, lens) {
            return Some(finish);
        }
        lens.pop();
    }
    None
}

/// `_match_arguments_partial`: how many words each leading positional takes.
fn match_partial(slots: &[Slot], s: &[u8]) -> Vec<usize> {
    for n in (1..=slots.len()).rev() {
        let mut lens = Vec::new();
        if let Some(end) = match_slots(&slots[..n], s, 0, &mut lens) {
            // A positional that matched nothing must not use itself up before an option.
            if end < s.len() && s[end] == b'O' {
                while lens.last() == Some(&0) {
                    lens.pop();
                }
            }
            return lens;
        }
    }
    Vec::new()
}

/// The name an error message gives an argument (`_get_action_name`).
fn arg_name(arg: &Arg) -> String {
    if !arg.flags.is_empty() {
        return arg.flags.join("/");
    }
    if let Some(m) = &arg.metavar {
        return m.clone();
    }
    arg.dest.clone()
}

fn type_name(ty: Ty) -> &'static str {
    match ty {
        Ty::Str => "str",
        Ty::Int => "int",
        Ty::Float => "float",
    }
}

/// Converts and checks one word (`_get_value` and `_check_value`).
fn convert(arg: &Arg, text: &str) -> Result<Val, String> {
    let value = match arg.ty {
        Ty::Str => Val::Str(text.to_owned()),
        Ty::Int => Val::Int(parse_int(text).ok_or_else(|| invalid_value(arg, text))?),
        Ty::Float => Val::Float(parse_float(text).ok_or_else(|| invalid_value(arg, text))?),
    };
    check_choice(arg, &value)?;
    Ok(value)
}

fn invalid_value(arg: &Arg, text: &str) -> String {
    format!("argument {}: invalid {} value: {}", arg_name(arg), type_name(arg.ty), repr_str(text))
}

fn check_choice(arg: &Arg, value: &Val) -> Result<(), String> {
    if arg.choices.is_empty() {
        return Ok(());
    }
    let allowed = match value {
        Val::Str(s) => arg.choices.iter().any(|c| c == s),
        Val::Int(n) => arg.choices.iter().any(|c| parse_int(c) == Some(*n)),
        Val::Float(f) => arg.choices.iter().any(|c| parse_float(c) == Some(*f)),
        _ => true,
    };
    if allowed {
        return Ok(());
    }
    // Python 3.14 shows the word and the choices as quoted text whatever their type.
    let shown = match value {
        Val::Str(s) => repr_str(s),
        Val::Int(n) => repr_str(&n.to_string()),
        Val::Float(f) => repr_str(&super::pyfmt::float_repr(*f)),
        _ => String::new(),
    };
    let list: Vec<String> = arg.choices.iter().map(|c| repr_str(c)).collect();
    Err(format!("argument {}: invalid choice: {shown} (choose from {})", arg_name(arg), list.join(", ")))
}

struct Parser<'a, 'b> {
    cmd: &'a Command,
    prog: &'a str,
    args: Vec<Arg>,
    sinks: &'a mut Sinks<'b>,
}

/// `parse_known_args` of one parser.
fn parse_known(
    cmd: &Command,
    prog: &str,
    args: &[String],
    matches: &mut Matches,
    extras: &mut Vec<String>,
    sinks: &mut Sinks<'_>,
) -> Result<(), Stop> {
    let mut parser = Parser { cmd, prog, args: cmd.all_args(), sinks };
    parser.run(args, matches, extras)
}

impl Parser<'_, '_> {
    fn fail(&mut self, message: &str) -> Stop {
        error(self.cmd, self.prog, message, self.sinks)
    }

    fn run(
        &mut self,
        strings: &[String],
        matches: &mut Matches,
        extras: &mut Vec<String>,
    ) -> Result<(), Stop> {
        // Defaults first, so an absent argument reads as its default.
        for arg in &self.args {
            if !matches!(arg.action, Action::Help | Action::Version(_)) {
                matches.set(&arg.dest, arg.default.clone());
            }
        }
        if let Some(subs) = &self.cmd.subs {
            matches.set(&subs.dest, Val::None);
        }
        let (pattern, found) = self.classify_all(strings)?;
        // The positionals still waiting: declared ones, then the subcommand slot.
        let mut waiting: Vec<Waiting> = self
            .args
            .iter()
            .enumerate()
            .filter(|(_, a)| a.is_positional())
            .map(|(i, _)| Waiting::Declared(i))
            .collect();
        if self.cmd.subs.is_some() {
            waiting.push(Waiting::Subcommand);
        }
        let max_option = found.iter().rposition(Option::is_some);
        let mut walk = Walk { strings, pattern, found, waiting, seen: Vec::new(), matches, extras };

        let mut start = 0usize;
        if let Some(max_option) = max_option {
            while start <= max_option {
                let mut next_option = start;
                while next_option <= max_option && walk.found[next_option].is_none() {
                    next_option += 1;
                }
                if start != next_option {
                    let end = self.consume_positionals(&mut walk, start)?;
                    if end > start {
                        start = end;
                        continue;
                    }
                    start = end;
                }
                if walk.found.get(start).is_none_or(Option::is_none) {
                    walk.extras.extend(strings[start..next_option].iter().cloned());
                    start = next_option;
                }
                start = self.consume_optional(&mut walk, start)?;
            }
        }
        let stop = self.consume_positionals(&mut walk, start)?;
        walk.extras.extend(strings[stop..].iter().cloned());

        // Required positionals that never matched.
        let missing: Vec<String> = walk
            .waiting
            .iter()
            .filter_map(|w| match w {
                Waiting::Declared(i) if self.args[*i].required() && !walk.seen.contains(i) => {
                    Some(arg_name(&self.args[*i]))
                }
                _ => None,
            })
            .collect();
        if !missing.is_empty() {
            return Err(self.fail(&format!("the following arguments are required: {}", missing.join(", "))));
        }
        Ok(())
    }

    /// Marks every word as an option string (`O`), a plain word (`A`) or the `--` that ends the
    /// options (`-`, after which everything is a word).
    fn classify_all(&mut self, strings: &[String]) -> Result<(Vec<u8>, Vec<Option<Found>>), Stop> {
        let options = Options { args: &self.args };
        let mut pattern: Vec<u8> = Vec::with_capacity(strings.len());
        let mut found: Vec<Option<Found>> = vec![None; strings.len()];
        let mut after_dashes = false;
        for (i, text) in strings.iter().enumerate() {
            if after_dashes {
                pattern.push(b'A');
            } else if text == "--" {
                pattern.push(b'-');
                after_dashes = true;
            } else {
                match classify(text, &options) {
                    Ok(None) => pattern.push(b'A'),
                    Ok(Some(f)) => {
                        found[i] = Some(f);
                        pattern.push(b'O');
                    }
                    Err(message) => return Err(self.fail(&message)),
                }
            }
        }
        Ok((pattern, found))
    }

    fn consume_positionals(&mut self, walk: &mut Walk<'_, '_>, start: usize) -> Result<usize, Stop> {
        let slots: Vec<Slot> = walk
            .waiting
            .iter()
            .map(|w| match w {
                Waiting::Subcommand => Slot::Subcommand,
                Waiting::Declared(i) => match self.args[*i].nargs {
                    Nargs::One => Slot::One,
                    Nargs::Optional => Slot::Optional,
                    Nargs::ZeroOrMore => Slot::ZeroOrMore,
                    Nargs::OneOrMore => Slot::OneOrMore,
                },
            })
            .collect();
        let counts = match_partial(&slots, &walk.pattern[start..]);
        let mut at = start;
        for (w, count) in walk.waiting.clone().iter().zip(counts.iter()) {
            let mut words: Vec<String> = walk.strings[at..at + count].to_vec();
            // The first `--` is dropped from the words it separates.
            if walk.pattern[at..at + count].contains(&b'-') {
                remove_first_dashes(&mut words);
            }
            at += count;
            match w {
                Waiting::Declared(i) => {
                    let value = self.positional_value(*i, &words)?;
                    walk.matches.set(&self.args[*i].dest, value);
                    walk.seen.push(*i);
                }
                Waiting::Subcommand => self.run_subcommand(&words, walk)?,
            }
        }
        walk.waiting.drain(..counts.len().min(walk.waiting.len()));
        Ok(at)
    }

    /// `_get_values` for a positional.
    fn positional_value(&mut self, index: usize, words: &[String]) -> Result<Val, Stop> {
        let arg = self.args[index].clone();
        let result: Result<Val, String> = (|| match arg.nargs {
            Nargs::One => {
                let word = words.first().ok_or_else(|| "expected one argument".to_owned())?;
                convert(&arg, word)
            }
            Nargs::Optional => match words.first() {
                Some(word) => convert(&arg, word),
                None => default_value(&arg),
            },
            Nargs::ZeroOrMore if words.is_empty() => match &arg.default {
                Val::None => Ok(Val::List(Vec::new())),
                _ => default_value(&arg),
            },
            Nargs::ZeroOrMore | Nargs::OneOrMore => {
                for w in words {
                    convert(&arg, w)?;
                }
                Ok(Val::List(words.to_vec()))
            }
        })();
        result.map_err(|m| self.fail(&m))
    }

    fn run_subcommand(&mut self, words: &[String], walk: &mut Walk<'_, '_>) -> Result<(), Stop> {
        let Some(subs) = &self.cmd.subs else { return Ok(()) };
        let Some(name) = words.first() else { return Ok(()) };
        let shown = subs.metavar.clone().unwrap_or_else(|| subs.dest.clone());
        let Some(sub) = subs.commands.iter().find(|c| &c.name == name) else {
            let list: Vec<String> = subs.commands.iter().map(|c| repr_str(&c.name)).collect();
            let message = format!(
                "argument {shown}: invalid choice: {} (choose from {})",
                repr_str(name),
                list.join(", ")
            );
            return Err(self.fail(&message));
        };
        walk.matches.set(&subs.dest, Val::Str(name.clone()));
        walk.matches.path.push(name.clone());
        let prog = format!("{} {}", self.prog, name);
        let mut rest_extras = Vec::new();
        parse_known(sub, &prog, &words[1..], walk.matches, &mut rest_extras, self.sinks)?;
        walk.extras.extend(rest_extras);
        Ok(())
    }

    fn consume_optional(&mut self, walk: &mut Walk<'_, '_>, start: usize) -> Result<usize, Stop> {
        let Some(mut current) = walk.found[start].clone() else { return Ok(start + 1) };
        let mut taken: Vec<(usize, Vec<String>)> = Vec::new();
        let stop;
        loop {
            let Some(index) = current.arg else {
                walk.extras.push(walk.strings[start].clone());
                return Ok(start + 1);
            };
            let arg = self.args[index].clone();
            let Some(explicit) = current.explicit.clone() else {
                let (words, end) = self.option_words(&arg, start + 1, walk)?;
                taken.push((index, words));
                stop = end;
                break;
            };
            let single_dash = !current.flag.starts_with("--");
            if arg.takes_nothing() && single_dash && !explicit.is_empty() {
                // `-hV`: this flag takes nothing, so the rest of the word may be more short flags.
                taken.push((index, Vec::new()));
                let next_flag = format!("-{}", explicit.chars().next().unwrap_or(' '));
                let options = Options { args: &self.args };
                let Some(next) = options.exact(&next_flag) else {
                    let message = format!(
                        "argument {}: ignored explicit argument {}",
                        arg_name(&arg),
                        repr_str(&explicit)
                    );
                    return Err(self.fail(&message));
                };
                let rest: String = explicit.chars().skip(1).collect();
                current =
                    Found { arg: Some(next), flag: next_flag, explicit: (!rest.is_empty()).then_some(rest) };
            } else if arg.takes_nothing() {
                let message =
                    format!("argument {}: ignored explicit argument {}", arg_name(&arg), repr_str(&explicit));
                return Err(self.fail(&message));
            } else {
                taken.push((index, vec![explicit]));
                stop = start + 1;
                break;
            }
        }
        for (index, words) in taken {
            self.take_option(index, &words, walk)?;
        }
        Ok(stop)
    }

    /// The words an option takes from what follows it (`_match_argument`).
    fn option_words(
        &mut self,
        arg: &Arg,
        from: usize,
        walk: &Walk<'_, '_>,
    ) -> Result<(Vec<String>, usize), Stop> {
        if arg.takes_nothing() {
            return Ok((Vec::new(), from));
        }
        if walk.pattern.get(from) == Some(&b'A') {
            return Ok((vec![walk.strings[from].clone()], from + 1));
        }
        let message = format!("argument {}: expected one argument", arg_name(arg));
        Err(self.fail(&message))
    }

    fn take_option(&mut self, index: usize, words: &[String], walk: &mut Walk<'_, '_>) -> Result<(), Stop> {
        let arg = self.args[index].clone();
        walk.seen.push(index);
        match &arg.action {
            Action::Help => {
                let text = format_help(self.cmd, self.prog, self.sinks.columns);
                let _ = self.sinks.out.write_all(text.as_bytes());
                Err(Stop::Exit(0))
            }
            Action::Version(text) => {
                let _ = writeln!(self.sinks.out, "{text}");
                Err(Stop::Exit(0))
            }
            Action::StoreTrue => {
                walk.matches.set(&arg.dest, Val::Bool(true));
                Ok(())
            }
            Action::Store | Action::Append => {
                let word = words.first().cloned().unwrap_or_default();
                let value = convert(&arg, &word).map_err(|m| self.fail(&m))?;
                if arg.action == Action::Append {
                    let mut list = walk.matches.list(&arg.dest);
                    if let Val::Str(s) = &value {
                        list.push(s.clone());
                    }
                    walk.matches.set(&arg.dest, Val::List(list));
                } else {
                    walk.matches.set(&arg.dest, value);
                }
                Ok(())
            }
        }
    }
}

/// Where a parse of one parser stands: the words and how they were classified, the positionals
/// still waiting, and what has been collected so far.
struct Walk<'s, 'm> {
    strings: &'s [String],
    pattern: Vec<u8>,
    found: Vec<Option<Found>>,
    waiting: Vec<Waiting>,
    seen: Vec<usize>,
    matches: &'m mut Matches,
    extras: &'m mut Vec<String>,
}

/// A positional (or the subcommand slot) that has not been matched yet.
#[derive(Clone)]
enum Waiting {
    Declared(usize),
    Subcommand,
}

fn remove_first_dashes(words: &mut Vec<String>) {
    if let Some(at) = words.iter().position(|w| w == "--") {
        words.remove(at);
    }
}

/// The default of a positional that was not given, converted and checked like a typed word.
fn default_value(arg: &Arg) -> Result<Val, String> {
    match &arg.default {
        Val::Str(s) => convert(arg, s),
        other => Ok(other.clone()),
    }
}
