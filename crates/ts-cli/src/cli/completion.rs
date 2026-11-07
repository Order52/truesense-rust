//! Shell completion scripts, generated from the command tree so that they never go stale.
//!
//! The scripts are those of the Python edition (`completion.py`): bash and zsh complete areas,
//! verbs, the words a positional takes and the long options; fish completes areas and verbs.
#![forbid(unsafe_code)]

use super::argparse::Command;

/// What the completions know about one parser.
struct Node<'a> {
    /// The long options (`--help`, `--json`, ...).
    opts: Vec<String>,
    /// The subcommands.
    subs: Vec<(&'a str, Node<'a>)>,
    /// The words the positionals take, in order.
    choices: Vec<String>,
}

fn node(cmd: &Command) -> Node<'_> {
    let mut opts = Vec::new();
    let mut choices = Vec::new();
    for arg in cmd.all_args() {
        opts.extend(arg.flags.iter().filter(|f| f.starts_with("--")).cloned());
        if arg.is_positional() && !arg.choices.is_empty() {
            choices.extend(arg.choices.iter().cloned());
        }
    }
    let subs = cmd
        .subs
        .as_ref()
        .map(|s| s.commands.iter().map(|c| (c.name.as_str(), node(c))).collect())
        .unwrap_or_default();
    Node { opts, subs, choices }
}

/// The script for `shell` (`bash`, `zsh` or `fish`); `None` for any other word.
pub(super) fn generate(root: &Command, shell: &str) -> Option<String> {
    let tree = node(root);
    match shell {
        "fish" => {
            let mut lines = vec!["complete -c truesense -f".to_owned()];
            for (area, sub) in &tree.subs {
                lines.push(format!("complete -c truesense -n '__fish_use_subcommand' -a {area}"));
                let verbs = sub.subs.iter().map(|(v, _)| (*v).to_owned()).chain(sub.choices.iter().cloned());
                for verb in verbs {
                    lines.push(format!(
                        "complete -c truesense -n '__fish_seen_subcommand_from {area}' -a '{verb}'"
                    ));
                }
            }
            Some(lines.join("\n"))
        }
        "bash" | "zsh" => {
            let mut cases = Vec::new();
            for (area, sub) in &tree.subs {
                let words: Vec<String> = sub
                    .subs
                    .iter()
                    .map(|(v, _)| (*v).to_owned())
                    .chain(sub.choices.iter().cloned())
                    .chain(sub.opts.iter().cloned())
                    .collect();
                let mut deep = Vec::new();
                for (verb, s2) in &sub.subs {
                    let w2: Vec<&str> = s2.choices.iter().chain(s2.opts.iter()).map(String::as_str).collect();
                    if !w2.is_empty() {
                        deep.push(format!(
                            "        {verb}) COMPREPLY=( $(compgen -W \"{}\" -- \"$cur\") ); return;;",
                            w2.join(" ")
                        ));
                    }
                }
                cases.push(format!(
                    "    {area}) if [ \"$COMP_CWORD\" -eq 2 ]; then COMPREPLY=( $(compgen -W \"{}\" -- \"$cur\") ); else case \"${{COMP_WORDS[2]}}\" in\n{}\n        *) COMPREPLY=( $(compgen -W \"--json\" -- \"$cur\") );; esac; fi;;",
                    words.join(" "),
                    deep.join("\n")
                ));
            }
            let areas: Vec<&str> = tree.subs.iter().map(|(a, _)| *a).collect();
            let body = format!(
                "_truesense() {{\n  local cur=\"${{COMP_WORDS[COMP_CWORD]}}\"\n  if [ \"$COMP_CWORD\" -eq 1 ]; then COMPREPLY=( $(compgen -W \"{} --json --help --version\" -- \"$cur\") ); return; fi\n  case \"${{COMP_WORDS[1]}}\" in\n{}\n  esac\n}}\ncomplete -F _truesense truesense",
                areas.join(" "),
                cases.join("\n")
            );
            Some(if shell == "bash" {
                body
            } else {
                format!("autoload -U +X bashcompinit && bashcompinit\n{body}")
            })
        }
        _ => None,
    }
}
