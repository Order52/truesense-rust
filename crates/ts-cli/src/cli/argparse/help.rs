//! The usage line and the help page, formatted the way `argparse.HelpFormatter` does it.
//!
//! The width is the terminal width minus two, and the help column is at most 24; a usage line
//! that does not fit is split into the options and the positionals, each part kept whole.
#![forbid(unsafe_code)]

use super::{Arg, Command, Help, Nargs, textwrap};

/// The widest column the help text may start at.
const MAX_HELP_POSITION: usize = 24;
const PREFIX: &str = "usage: ";

/// The metavar (or the braces of the choices) of an argument.
fn metavar_of(arg: &Arg, default: &str) -> String {
    if let Some(m) = &arg.metavar {
        return m.clone();
    }
    if !arg.choices.is_empty() {
        return format!("{{{}}}", arg.choices.join(","));
    }
    default.to_owned()
}

/// `_format_args`: how the words of an argument are written in usage and help.
fn format_args(arg: &Arg, default: &str) -> String {
    let m = metavar_of(arg, default);
    match arg.nargs {
        Nargs::One => m,
        Nargs::Optional => format!("[{m}]"),
        Nargs::ZeroOrMore => format!("[{m} ...]"),
        Nargs::OneOrMore => format!("{m} [{m} ...]"),
    }
}

/// The text of the subcommand slot: the metavar or the braces, then `...`.
fn subs_metavar(cmd: &Command) -> Option<String> {
    let subs = cmd.subs.as_ref()?;
    Some(subs.metavar.clone().unwrap_or_else(|| {
        format!("{{{}}}", subs.commands.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(","))
    }))
}

/// One argument as it appears in the usage line, or `None` when it is hidden.
fn usage_part(arg: &Arg) -> Option<String> {
    if arg.help == Help::Suppress {
        return None;
    }
    if arg.is_positional() {
        return Some(format_args(arg, &arg.dest));
    }
    let first = arg.flags.first()?;
    let text = if arg.takes_nothing() {
        first.clone()
    } else {
        format!("{first} {}", format_args(arg, &arg.dest.to_uppercase()))
    };
    Some(format!("[{text}]"))
}

fn get_lines(parts: &[String], indent: &str, prefix: Option<&str>, text_width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line: Vec<&str> = Vec::new();
    let indent_len = indent.chars().count();
    let mut line_len = match prefix {
        Some(p) => p.chars().count().saturating_sub(1),
        None => indent_len.saturating_sub(1),
    };
    for part in parts {
        let plen = part.chars().count();
        if line_len + 1 + plen > text_width && !line.is_empty() {
            lines.push(format!("{indent}{}", line.join(" ")));
            line.clear();
            line_len = indent_len.saturating_sub(1);
        }
        line.push(part);
        line_len += plen + 1;
    }
    if !line.is_empty() {
        lines.push(format!("{indent}{}", line.join(" ")));
    }
    if prefix.is_some() {
        if let Some(first) = lines.first_mut() {
            *first = first.chars().skip(indent_len).collect();
        }
    }
    lines
}

/// The `usage: ...` block (with its trailing blank line) for a parser.
///
/// Every argument is one unbreakable part (`[--color COLOR]`, `items [items ...]`,
/// `{a,b} ...`); a line that does not fit is split between the parts.
fn usage_block(cmd: &Command, prog: &str, width: usize) -> String {
    let all = cmd.all_args();
    let opt_parts: Vec<String> = all.iter().filter(|a| !a.is_positional()).filter_map(usage_part).collect();
    let mut pos_parts: Vec<String> =
        all.iter().filter(|a| a.is_positional()).filter_map(usage_part).collect();
    if let Some(m) = subs_metavar(cmd) {
        pos_parts.push(format!("{m} ..."));
    }
    let action_usage = opt_parts.iter().chain(pos_parts.iter()).cloned().collect::<Vec<_>>().join(" ");
    let usage =
        [prog, action_usage.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" ");
    let text_width = width;
    if PREFIX.len() + usage.chars().count() <= text_width {
        return format!("{PREFIX}{usage}\n\n");
    }
    let prog_len = prog.chars().count();
    let lines = if (PREFIX.len() + prog_len) as f64 <= 0.75 * text_width as f64 {
        // A short program name: the parts follow it, positionals on lines of their own.
        let indent = " ".repeat(PREFIX.len() + prog_len + 1);
        if !opt_parts.is_empty() {
            let mut first = vec![prog.to_owned()];
            first.extend(opt_parts);
            let mut lines = get_lines(&first, &indent, Some(PREFIX), text_width);
            lines.extend(get_lines(&pos_parts, &indent, None, text_width));
            lines
        } else if !pos_parts.is_empty() {
            let mut first = vec![prog.to_owned()];
            first.extend(pos_parts);
            get_lines(&first, &indent, Some(PREFIX), text_width)
        } else {
            vec![prog.to_owned()]
        }
    } else {
        // A very long program name: the parts go below it, options and positionals apart when
        // everything together needs more than one line.
        let indent = " ".repeat(PREFIX.len());
        let mut all_parts = opt_parts.clone();
        all_parts.extend(pos_parts.clone());
        let mut lines = get_lines(&all_parts, &indent, None, text_width);
        if lines.len() > 1 {
            lines = get_lines(&opt_parts, &indent, None, text_width);
            lines.extend(get_lines(&pos_parts, &indent, None, text_width));
        }
        lines.insert(0, prog.to_owned());
        lines
    };
    format!("{PREFIX}{}\n\n", lines.join("\n"))
}

/// What the help column needs to know about one listed argument.
struct Row {
    invocation: String,
    help: Help,
    children: Vec<(String, Help)>,
}

fn invocation(arg: &Arg) -> String {
    if arg.is_positional() {
        return metavar_of(arg, &arg.dest);
    }
    if arg.takes_nothing() {
        return arg.flags.join(", ");
    }
    let args = format_args(arg, &arg.dest.to_uppercase());
    arg.flags.iter().map(|f| format!("{f} {args}")).collect::<Vec<_>>().join(", ")
}

/// The listed positionals and options of a parser.
fn rows(cmd: &Command) -> (Vec<Row>, Vec<Row>) {
    let all = cmd.all_args();
    let mut positionals = Vec::new();
    let mut options = Vec::new();
    for arg in &all {
        if arg.help == Help::Suppress {
            continue;
        }
        let row = Row { invocation: invocation(arg), help: arg.help.clone(), children: Vec::new() };
        if arg.is_positional() {
            positionals.push(row);
        } else {
            options.push(row);
        }
    }
    if let (Some(subs), Some(m)) = (cmd.subs.as_ref(), subs_metavar(cmd)) {
        let children = subs
            .commands
            .iter()
            .filter(|c| c.help != Help::None)
            .map(|c| (c.name.clone(), c.help.clone()))
            .collect();
        positionals.push(Row { invocation: m, help: Help::None, children });
    }
    (positionals, options)
}

fn help_text(help: &Help) -> Option<String> {
    match help {
        Help::Text(t) if !t.trim().is_empty() => Some(t.clone()),
        Help::Suppress => Some("==SUPPRESS==".to_owned()),
        _ => None,
    }
}

fn format_row(
    out: &mut String,
    invocation: &str,
    help: &Help,
    indent: usize,
    help_position: usize,
    width: usize,
) {
    let help_width = width.saturating_sub(help_position).max(11);
    let action_width = help_position.saturating_sub(indent + 2);
    let text = help_text(help);
    let mut first_indent = 0;
    match &text {
        None => {
            out.push_str(&format!("{:indent$}{invocation}\n", ""));
        }
        Some(_) if invocation.chars().count() <= action_width => {
            out.push_str(&format!("{:indent$}{invocation:<action_width$}  ", ""));
        }
        Some(_) => {
            out.push_str(&format!("{:indent$}{invocation}\n", ""));
            first_indent = help_position;
        }
    }
    if let Some(text) = text {
        let normalised = text.split_whitespace().collect::<Vec<_>>().join(" ");
        let lines = textwrap::wrap(&normalised, help_width);
        if let Some(first) = lines.first() {
            out.push_str(&format!("{:first_indent$}{first}\n", ""));
        }
        for line in lines.iter().skip(1) {
            out.push_str(&format!("{:help_position$}{line}\n", ""));
        }
    }
}

fn section(out: &mut String, title: &str, rows: &[Row], help_position: usize, width: usize) {
    if rows.is_empty() {
        return;
    }
    out.push_str(&format!("{title}:\n"));
    for row in rows {
        format_row(out, &row.invocation, &row.help, 2, help_position, width);
        for (name, help) in &row.children {
            format_row(out, name, help, 4, help_position, width);
        }
    }
    out.push('\n');
}

/// The `usage:` line alone, as printed above an error message.
pub(crate) fn format_usage(cmd: &Command, prog: &str, columns: usize) -> String {
    let width = columns.saturating_sub(2).max(1);
    let block = usage_block(cmd, prog, width);
    format!("{}\n", block.trim_matches('\n'))
}

/// The whole help page of a parser.
pub(crate) fn format_help(cmd: &Command, prog: &str, columns: usize) -> String {
    let width = columns.saturating_sub(2).max(1);
    let (positionals, options) = rows(cmd);
    let mut longest = 0;
    for row in positionals.iter().chain(options.iter()) {
        let mut l = row.invocation.chars().count() + 2;
        for (name, _) in &row.children {
            l = l.max(name.chars().count() + 2);
        }
        longest = longest.max(l);
    }
    let help_position = (longest + 2).min(MAX_HELP_POSITION.min(width.saturating_sub(20).max(4)));
    let mut out = usage_block(cmd, prog, width);
    if let Some(d) = &cmd.description {
        out.push_str(d);
        out.push_str("\n\n");
    }
    section(&mut out, "positional arguments", &positionals, help_position, width);
    section(&mut out, "options", &options, help_position, width);
    if let Some(e) = &cmd.epilog {
        out.push_str(e);
        out.push_str("\n\n");
    }
    format!("{}\n", out.trim_matches('\n'))
}
