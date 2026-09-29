//! Renders `--help` from the command tree.

use crate::args::{Arg, GLOBAL_OPTS, JSON_OPT, Opt, Spec, command_line};
use crate::context::Style;

const WIDTH: usize = 92;

/// The help page for a command.
pub fn render(spec: &Spec, path: &[&str], style: Style) -> String {
    let mut out = String::new();
    let is_root = path.is_empty();
    if is_root {
        out.push_str(&style.bold(&format!("beskar {}", env!("CARGO_PKG_VERSION"))));
        out.push('\n');
    }
    out.push_str(&style.bold(spec.about));
    out.push('\n');
    if !spec.details.is_empty() {
        out.push('\n');
        out.push_str(&wrap(spec.details, WIDTH, 0));
        out.push('\n');
    }

    out.push('\n');
    out.push_str(&style.bold("USAGE"));
    out.push_str("\n  ");
    out.push_str(&usage(spec, path));
    out.push('\n');

    if !spec.subs.is_empty() {
        out.push('\n');
        out.push_str(&style.bold("COMMANDS"));
        out.push('\n');
        let width = spec.subs.iter().map(|s| s.name.len()).max().unwrap_or(0);
        for sub in spec.subs {
            let name = format!("{:<width$}", sub.name);
            out.push_str(&format!("  {}  {}\n", style.cyan(&name), sub.about));
        }
        if let Some(default) = spec.default_sub {
            out.push_str(&style.dim(&format!(
                "\n  Without a command, '{}' runs '{default}'.\n",
                command_line(path)
            )));
        }
    }

    if !spec.args.is_empty() {
        out.push('\n');
        out.push_str(&style.bold("ARGUMENTS"));
        out.push('\n');
        let labels: Vec<String> = spec.args.iter().map(arg_label).collect();
        let width = labels.iter().map(|l| l.len()).max().unwrap_or(0);
        for (arg, label) in spec.args.iter().zip(&labels) {
            out.push_str(&format!("  {label:<width$}  {}\n", arg.help));
        }
    }

    let mut options: Vec<Opt> = spec.opts.to_vec();
    if spec.json {
        options.push(JSON_OPT);
    }
    if !options.is_empty() {
        out.push('\n');
        out.push_str(&style.bold("OPTIONS"));
        out.push('\n');
        out.push_str(&options_block(&options));
    }

    out.push('\n');
    out.push_str(&style.bold("GLOBAL OPTIONS"));
    out.push('\n');
    let mut globals = GLOBAL_OPTS.to_vec();
    if is_root {
        globals.push(Opt {
            long: "version",
            short: Some('V'),
            value: None,
            help: "Print the version",
        });
    }
    out.push_str(&options_block(&globals));

    if !spec.examples.is_empty() {
        out.push('\n');
        out.push_str(&style.bold("EXAMPLES"));
        out.push('\n');
        for example in spec.examples {
            out.push_str(&format!("  {example}\n"));
        }
    }
    out
}

/// `beskar repo update [OPTIONS] [PATH]`
pub fn usage(spec: &Spec, path: &[&str]) -> String {
    let mut line = command_line(path);
    if !spec.subs.is_empty() {
        line.push_str(if spec.default_sub.is_some() {
            " [COMMAND]"
        } else {
            " <COMMAND>"
        });
    }
    if !spec.opts.is_empty() || spec.json {
        line.push_str(" [OPTIONS]");
    }
    for arg in spec.args {
        line.push(' ');
        line.push_str(&arg_label(arg));
    }
    line
}

fn arg_label(arg: &Arg) -> String {
    let (open, close) = if arg.required { ('<', '>') } else { ('[', ']') };
    format!(
        "{open}{}{close}{}",
        arg.name,
        if arg.many { "..." } else { "" }
    )
}

fn options_block(options: &[Opt]) -> String {
    let labels: Vec<String> = options
        .iter()
        .map(|o| {
            let short = o
                .short
                .map_or_else(|| "    ".to_string(), |c| format!("-{c}, "));
            let value = o.value.map_or(String::new(), |v| format!(" <{v}>"));
            format!("{short}--{}{value}", o.long)
        })
        .collect();
    let width = labels.iter().map(|l| l.len()).max().unwrap_or(0);
    let mut out = String::new();
    for (opt, label) in options.iter().zip(&labels) {
        let indent = 2 + width + 2;
        let text = wrap(opt.help, WIDTH.saturating_sub(indent), 0);
        let mut lines = text.lines();
        out.push_str(&format!(
            "  {label:<width$}  {}\n",
            lines.next().unwrap_or("")
        ));
        for line in lines {
            out.push_str(&format!("{}{line}\n", " ".repeat(indent)));
        }
    }
    out
}

/// Wraps paragraphs to `width`. Lines that start with a space are kept as they are.
pub fn wrap(text: &str, width: usize, indent: usize) -> String {
    let pad = " ".repeat(indent);
    let mut lines: Vec<String> = Vec::new();
    for paragraph in text.split('\n') {
        if paragraph.is_empty() || paragraph.starts_with(' ') {
            lines.push(format!("{pad}{paragraph}").trim_end().to_string());
            continue;
        }
        let mut current = String::new();
        for word in paragraph.split_whitespace() {
            if !current.is_empty() && current.chars().count() + 1 + word.chars().count() > width {
                lines.push(format!("{pad}{current}"));
                current.clear();
            }
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
        }
        lines.push(format!("{pad}{current}"));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{ROOT, lookup};

    fn page(command: &str) -> String {
        let names: Vec<String> = command.split_whitespace().map(String::from).collect();
        let (path, spec) = lookup(&names).unwrap();
        render(spec, &path, Style::new(false))
    }

    #[test]
    fn usage_lines_show_required_and_optional_arguments() {
        assert_eq!(
            usage(
                lookup(&["repo".into(), "update".into()]).unwrap().1,
                &["repo", "update"]
            ),
            "beskar repo update [OPTIONS] [PATH]"
        );
        assert_eq!(
            usage(
                lookup(&["repo".into(), "enable".into()]).unwrap().1,
                &["repo", "enable"]
            ),
            "beskar repo enable [OPTIONS] <PROFILE>..."
        );
        assert_eq!(
            usage(
                lookup(&["profile".into(), "create".into()]).unwrap().1,
                &["profile", "create"]
            ),
            "beskar profile create [OPTIONS] <NAME> [SKILL]..."
        );
        assert_eq!(
            usage(lookup(&["repo".into()]).unwrap().1, &["repo"]),
            "beskar repo <COMMAND>"
        );
        assert_eq!(
            usage(lookup(&["config".into()]).unwrap().1, &["config"]),
            "beskar config [COMMAND]"
        );
        assert_eq!(usage(&ROOT, &[]), "beskar <COMMAND>");
    }

    #[test]
    fn a_leaf_page_lists_arguments_options_globals_and_examples() {
        let text = page("repo update");
        assert!(text.starts_with("Install what the enabled profiles ask for\n\n"));
        assert!(text.contains("USAGE\n  beskar repo update [OPTIONS] [PATH]\n"));
        assert!(text.contains("ARGUMENTS\n  [PATH]  The repository"));
        assert!(text.contains("-n, --dry-run"));
        assert!(text.contains("    --on-conflict <POLICY>"));
        assert!(text.contains("    --json"));
        assert!(text.contains("GLOBAL OPTIONS\n"));
        assert!(text.contains("--home <PATH>"));
        assert!(text.contains("EXAMPLES\n  beskar repo update\n"));
        assert!(
            !text.contains("--version"),
            "only the top page shows --version"
        );
    }

    #[test]
    fn the_top_page_lists_every_command() {
        let text = render(&ROOT, &[], Style::new(false));
        assert!(text.starts_with(&format!("beskar {}\n", env!("CARGO_PKG_VERSION"))));
        for name in [
            "init", "doctor", "config", "library", "profile", "repo", "registry", "skill",
            "status", "update", "help",
        ] {
            assert!(text.contains(&format!("\n  {name}")), "missing {name}");
        }
        assert!(text.contains("-V, --version"));
        assert!(text.contains("beskar help format"));
    }

    #[test]
    fn lists_inside_help_text_keep_one_item_per_line() {
        let top = render(&ROOT, &[], Style::new(false));
        assert!(
            top.contains(
                "\n  library    the skills you own\n  profile    a named set of skills, such as \"coding\" or \"research\"\n  repo       a workspace"
            ),
            "{top}"
        );
        let set = page("config set");
        assert!(
            set.contains("\n  library        folder that holds skills/ and profiles/\n  registry       file that records"),
            "{set}"
        );
        assert!(
            set.contains("\n  on-conflict    ask, fail, keep or replace\n"),
            "{set}"
        );
    }

    #[test]
    fn verbose_is_described_for_what_it_does_in_each_command() {
        let status = page("repo status");
        assert!(status.contains("COPY column"), "{status}");
        assert!(!status.contains("already up to date"), "{status}");
        let update = page("repo update");
        assert!(
            update.contains("Also list skills that need nothing"),
            "{update}"
        );
        assert_eq!(page("status"), page("status"), "rendering is deterministic");
        assert!(page("status").contains("COPY column"));
        assert!(page("registry update").contains("Also list skills that need nothing"));
    }

    #[test]
    fn group_pages_list_subcommands_and_mention_the_default() {
        let text = page("config");
        assert!(text.contains("COMMANDS\n  show"));
        assert!(text.contains("Without a command, 'beskar config' runs 'show'."));
        assert!(!page("repo").contains("Without a command"));
    }

    #[test]
    fn no_line_of_prose_is_absurdly_long() {
        fn check(spec: &Spec, path: &[&str]) {
            let text = render(spec, path, Style::new(false));
            for line in text.lines() {
                let preformatted = line.starts_with("  ") || line.is_empty();
                assert!(
                    preformatted || line.chars().count() <= WIDTH + 2,
                    "{path:?}: {line}"
                );
            }
            for sub in spec.subs {
                let mut next = path.to_vec();
                next.push(sub.name);
                check(sub, &next);
            }
        }
        check(&ROOT, &[]);
    }

    #[test]
    fn wrap_keeps_paragraphs_and_preformatted_lines() {
        let text = "one two three four five six\n\n  keep   this   as is\nlast line";
        assert_eq!(
            wrap(text, 12, 0),
            "one two\nthree four\nfive six\n\n  keep   this   as is\nlast line"
        );
        assert_eq!(wrap("a b", 80, 2), "  a b");
    }
}
