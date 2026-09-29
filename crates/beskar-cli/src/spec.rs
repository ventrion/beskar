//! Every command, option and help text in one tree.

use crate::args::{Arg, Opt, Spec};

const YES: Opt = Opt {
    long: "yes",
    short: Some('y'),
    value: None,
    help: "Do not ask for confirmation",
};

const FORCE_REPLACE: Opt = Opt {
    long: "force",
    short: None,
    value: None,
    help: "Replace a different skill that has the same id",
};

const DRY_RUN: Opt = Opt {
    long: "dry-run",
    short: Some('n'),
    value: None,
    help: "Show what would happen and change nothing",
};

const ON_CONFLICT: Opt = Opt {
    long: "on-conflict",
    short: None,
    value: Some("POLICY"),
    help: "What to do with skills that have local changes: ask, fail, keep or replace (default: the 'on-conflict' setting)",
};

const VERBOSE_UPDATE: Opt = Opt {
    long: "verbose",
    short: Some('v'),
    value: None,
    help: "Also list skills that need nothing and folders Beskar leaves alone, and name the profile behind each change",
};

const VERBOSE_STATUS: Opt = Opt {
    long: "verbose",
    short: Some('v'),
    value: None,
    help: "Add a COPY column with a short fingerprint of each installed copy, and list folders Beskar leaves alone",
};

const ALL: Opt = Opt {
    long: "all",
    short: None,
    value: None,
    help: "Do this for every registered repository",
};

const REPO: Opt = Opt {
    long: "repo",
    short: Some('r'),
    value: Some("PATH"),
    help: "The repository (default: the registered repository that contains the current folder)",
};

const SKILL_ARG: Arg = Arg {
    name: "SKILL",
    help: "A skill id, as shown by 'beskar library list'",
    required: true,
    many: false,
};

const PROFILE_ARG: Arg = Arg {
    name: "PROFILE",
    help: "A profile name, as shown by 'beskar profile list'",
    required: true,
    many: false,
};

const REPO_PATH: Arg = Arg {
    name: "PATH",
    help: "The repository (default: the registered repository that contains the current folder)",
    required: false,
    many: false,
};

const UPDATE_DETAILS: &str = "Makes the repository's skills folder match its enabled profiles: installs missing skills, brings changed ones up to date and removes the ones no profile asks for any more. \
Only skills that Beskar installed are ever removed; folders you made yourself are left alone.\n\n\
A skill whose installed copy was edited is a conflict, and Beskar never overwrites it silently. \
At a terminal it asks what to do. Elsewhere the --on-conflict policy decides: 'fail' stops before changing anything, 'keep' leaves the edited skills alone, 'replace' overwrites them with the library version.\n\n\
Use --dry-run to see the plan first.";

pub static ROOT: Spec = Spec {
    name: "beskar",
    about: "Better Skill Arrangement: one library of agent skills, installed per repository",
    details: concat!(
        "Beskar keeps one curated library of agent skills and installs into each repository only the skills that repository needs.\n\n",
        "  library    the skills you own\n",
        "  profile    a named set of skills, such as \"coding\" or \"research\"\n",
        "  repo       a workspace where skills are installed, in .agents/skills\n",
        "  registry   everything Beskar manages on this machine\n\n",
        "Enabling a profile in a repository records what you want. Updating the repository installs it. ",
        "Installed skills are copies, and Beskar remembers what it installed, so it can tell a change in the library from an edit in the repository. ",
        "It never overwrites an edit without an explicit decision.\n\n",
        "Files that Beskar reads and writes use the bsk format. Run 'beskar help format' for its two-minute description."
    ),
    args: &[],
    opts: &[],
    subs: &[
        Spec {
            name: "init",
            about: "Set up Beskar: settings, registry and library folders",
            details: "Creates ~/.beskar with a settings file, an empty registry, and a library with skills/ and profiles/ folders. \
Running it again changes nothing that exists, so it also repairs a partial setup.\n\n\
Use --library to keep the library somewhere else, for example in a Git checkout you sync between machines.",
            opts: &[
                Opt {
                    long: "library",
                    short: None,
                    value: Some("PATH"),
                    help: "Keep the library in this folder",
                },
                Opt {
                    long: "force",
                    short: None,
                    value: None,
                    help: "Switch an existing setup to a different --library",
                },
            ],
            examples: &[
                "beskar init",
                "beskar init --library ~/dotfiles/skills-library",
            ],
            ..Spec::EMPTY
        },
        Spec {
            name: "doctor",
            about: "Check the setup and say what to fix",
            details: "Checks the settings, the library, the registry, every registered repository and what is installed in it. \
It only reads. It exits with status 1 if anything is broken; warnings alone leave the status at 0.",
            examples: &["beskar doctor", "beskar doctor --json"],
            json: true,
            ..Spec::EMPTY
        },
        Spec {
            name: "config",
            about: "Show or change the settings",
            details: "Settings live in config.bsk inside the Beskar folder. Without a subcommand, shows them.",
            subs: &[
                Spec {
                    name: "show",
                    about: "Show the settings in effect",
                    json: true,
                    ..Spec::EMPTY
                },
                Spec {
                    name: "set",
                    about: "Change one setting",
                    details: concat!(
                        "Edits the line in place and keeps every comment. Settings:\n\n",
                        "  library        folder that holds skills/ and profiles/\n",
                        "  registry       file that records repositories and installed skills\n",
                        "  agent-skills   where skills are installed inside a repository (default .agents/skills)\n",
                        "  on-conflict    ask, fail, keep or replace\n\n",
                        "Skills already installed under the old agent-skills folder stay where they are. Beskar stops managing them."
                    ),
                    args: &[
                        Arg {
                            name: "KEY",
                            help: "The setting's name",
                            required: true,
                            many: false,
                        },
                        Arg {
                            name: "VALUE",
                            help: "The new value",
                            required: true,
                            many: false,
                        },
                    ],
                    examples: &[
                        "beskar config set on-conflict keep",
                        "beskar config set agent-skills .claude/skills",
                    ],
                    ..Spec::EMPTY
                },
                Spec {
                    name: "path",
                    about: "Print the path of the settings file",
                    ..Spec::EMPTY
                },
            ],
            default_sub: Some("show"),
            ..Spec::EMPTY
        },
        Spec {
            name: "library",
            about: "Manage the skills you own",
            details: "The library is your curated collection: one folder per skill under skills/, one file per profile under profiles/. \
It is the source of truth for everything Beskar installs. It holds no machine-specific state, so you can keep it in Git.",
            subs: &[
                Spec {
                    name: "init",
                    about: "Create the library folders",
                    details: "'beskar init' already does this. Use it to recreate skills/ and profiles/ in a library that was cloned without its empty folders.",
                    ..Spec::EMPTY
                },
                Spec {
                    name: "add",
                    about: "Import one skill into the library",
                    details: "Copies a skill folder into the library. A skill is any folder; Beskar reads its name and description from SKILL.md when there is one.",
                    args: &[Arg {
                        name: "PATH",
                        help: "The skill's folder",
                        required: true,
                        many: false,
                    }],
                    opts: &[
                        Opt {
                            long: "name",
                            short: None,
                            value: Some("ID"),
                            help: "Import under this id instead of the folder's name",
                        },
                        FORCE_REPLACE,
                    ],
                    examples: &[
                        "beskar library add ~/Downloads/playwright",
                        "beskar library add ./my-skill --name code-review",
                    ],
                    ..Spec::EMPTY
                },
                Spec {
                    name: "scan",
                    about: "Find skills in a folder and offer to import them",
                    details: "Looks below PATH for folders that contain a SKILL.md and lists what it finds. \
Skills that are new are imported after you confirm. Skills already in the library with the same content are skipped. \
A skill that differs from the library's version is only replaced with --force.",
                    args: &[Arg {
                        name: "PATH",
                        help: "A folder that contains skills",
                        required: true,
                        many: false,
                    }],
                    opts: &[
                        YES,
                        Opt {
                            long: "force",
                            short: None,
                            value: None,
                            help: "Replace skills whose content differs from the library's",
                        },
                        DRY_RUN,
                    ],
                    examples: &[
                        "beskar library scan ~/Downloads/agent-skills",
                        "beskar library scan ~/skills --yes",
                    ],
                    json: true,
                    ..Spec::EMPTY
                },
                Spec {
                    name: "list",
                    about: "List the skills in the library",
                    examples: &["beskar library list"],
                    json: true,
                    ..Spec::EMPTY
                },
                Spec {
                    name: "show",
                    about: "Show one skill: metadata, files and where it is used",
                    args: &[SKILL_ARG],
                    examples: &["beskar library show playwright"],
                    json: true,
                    ..Spec::EMPTY
                },
                Spec {
                    name: "remove",
                    about: "Delete a skill from the library",
                    details: "Repositories that have the skill installed keep their copy until their next update, which removes it if it is untouched.",
                    args: &[SKILL_ARG],
                    opts: &[
                        Opt {
                            long: "force",
                            short: None,
                            value: None,
                            help: "Also take the skill out of the profiles that list it",
                        },
                        YES,
                    ],
                    examples: &["beskar library remove old-skill --yes"],
                    ..Spec::EMPTY
                },
            ],
            ..Spec::EMPTY
        },
        Spec {
            name: "profile",
            about: "Manage profiles, the named sets of skills",
            details: "A profile says which skills belong together. A repository can enable several profiles at once; \
its skills are the union of theirs, and a skill listed twice is installed once. \
Profiles live in the library as small files, one per profile.",
            subs: &[
                Spec {
                    name: "create",
                    about: "Create a profile",
                    args: &[
                        Arg {
                            name: "NAME",
                            help: "The new profile's name",
                            required: true,
                            many: false,
                        },
                        Arg {
                            name: "SKILL",
                            help: "Skills to start with",
                            required: false,
                            many: true,
                        },
                    ],
                    opts: &[Opt {
                        long: "description",
                        short: Some('d'),
                        value: Some("TEXT"),
                        help: "A line saying what the profile is for",
                    }],
                    examples: &[
                        "beskar profile create coding",
                        "beskar profile create coding git code-review testing -d \"Everyday engineering\"",
                    ],
                    ..Spec::EMPTY
                },
                Spec {
                    name: "delete",
                    about: "Delete a profile",
                    details: "Refuses while the profile is enabled in a registered repository, unless you pass --force.",
                    args: &[PROFILE_ARG],
                    opts: &[
                        Opt {
                            long: "force",
                            short: None,
                            value: None,
                            help: "Delete it even though repositories have it enabled",
                        },
                        YES,
                    ],
                    ..Spec::EMPTY
                },
                Spec {
                    name: "list",
                    about: "List the profiles",
                    json: true,
                    ..Spec::EMPTY
                },
                Spec {
                    name: "show",
                    about: "Show a profile's skills and where it is enabled",
                    args: &[PROFILE_ARG],
                    json: true,
                    ..Spec::EMPTY
                },
                Spec {
                    name: "add",
                    about: "Add skills to a profile",
                    details: "The skills must exist in the library. The rest of the profile file, including your comments, is left as it is.",
                    args: &[
                        PROFILE_ARG,
                        Arg {
                            name: "SKILL",
                            help: "Skills to add",
                            required: true,
                            many: true,
                        },
                    ],
                    examples: &["beskar profile add coding playwright code-review"],
                    ..Spec::EMPTY
                },
                Spec {
                    name: "remove",
                    about: "Remove skills from a profile",
                    args: &[
                        PROFILE_ARG,
                        Arg {
                            name: "SKILL",
                            help: "Skills to remove",
                            required: true,
                            many: true,
                        },
                    ],
                    ..Spec::EMPTY
                },
            ],
            ..Spec::EMPTY
        },
        Spec {
            name: "repo",
            about: "Manage repositories and what is installed in them",
            details: "A repository is any folder where agents run. It does not need to be a Git repository.\n\n\
Working with one takes three steps: register it ('add'), say which profiles it should have ('enable'), \
and install them ('update'). Enabling changes what you want. Updating changes the files.",
            subs: &[
                Spec {
                    name: "add",
                    about: "Register a repository",
                    details: "Registering records the folder in the registry. It installs nothing and changes no files.",
                    args: &[Arg {
                        name: "PATH",
                        help: "The repository's folder (default: the current folder)",
                        required: false,
                        many: false,
                    }],
                    examples: &["beskar repo add .", "beskar repo add ~/projects/api"],
                    ..Spec::EMPTY
                },
                Spec {
                    name: "remove",
                    about: "Stop managing a repository",
                    details: "By default the repository's files stay exactly as they are; Beskar just forgets it. \
With --purge it first removes the skills it installed there, following the same conflict rules as 'update'.",
                    args: &[REPO_PATH],
                    opts: &[
                        Opt {
                            long: "purge",
                            short: None,
                            value: None,
                            help: "Also remove the skills Beskar installed in it",
                        },
                        ON_CONFLICT,
                    ],
                    ..Spec::EMPTY
                },
                Spec {
                    name: "list",
                    about: "List registered repositories and their profiles",
                    json: true,
                    ..Spec::EMPTY
                },
                Spec {
                    name: "status",
                    about: "Show what is installed in a repository and how it compares with the library",
                    details: "For every skill the repository has or wants: whether it is up to date, whether the library has a newer version, and whether the installed copy was edited locally. \
Changes nothing.",
                    args: &[REPO_PATH],
                    opts: &[VERBOSE_STATUS],
                    json: true,
                    ..Spec::EMPTY
                },
                Spec {
                    name: "enable",
                    about: "Enable profiles in a repository",
                    details: "Records that the repository should have these profiles. Files are not touched until you run 'beskar repo update'.",
                    args: &[Arg {
                        name: "PROFILE",
                        help: "Profiles to enable",
                        required: true,
                        many: true,
                    }],
                    opts: &[REPO],
                    examples: &[
                        "beskar repo enable coding",
                        "beskar repo enable research frontend",
                    ],
                    ..Spec::EMPTY
                },
                Spec {
                    name: "disable",
                    about: "Disable profiles in a repository",
                    details: "Records that the repository should no longer have these profiles. Files are not touched until you run 'beskar repo update'.",
                    args: &[Arg {
                        name: "PROFILE",
                        help: "Profiles to disable",
                        required: true,
                        many: true,
                    }],
                    opts: &[REPO],
                    ..Spec::EMPTY
                },
                Spec {
                    name: "toggle",
                    about: "Flip profiles: enabled ones off, disabled ones on",
                    args: &[Arg {
                        name: "PROFILE",
                        help: "Profiles to flip",
                        required: true,
                        many: true,
                    }],
                    opts: &[REPO],
                    ..Spec::EMPTY
                },
                Spec {
                    name: "update",
                    about: "Install what the enabled profiles ask for",
                    details: UPDATE_DETAILS,
                    args: &[REPO_PATH],
                    opts: &[ALL, DRY_RUN, ON_CONFLICT, VERBOSE_UPDATE],
                    examples: &[
                        "beskar repo update",
                        "beskar repo update --dry-run",
                        "beskar repo update --all --on-conflict keep",
                    ],
                    json: true,
                    ..Spec::EMPTY
                },
            ],
            ..Spec::EMPTY
        },
        Spec {
            name: "registry",
            about: "Inspect and maintain everything Beskar manages on this machine",
            details: "The registry is machine-local state: which repositories exist, which profiles are enabled in each, \
and what Beskar installed there. It lives outside the library because it holds absolute paths.",
            subs: &[
                Spec {
                    name: "list",
                    about: "List repositories, or show where a profile or skill is used",
                    details: "Without options, lists every repository with its profiles. \
--profile answers \"where is this profile used?\". --skill answers \"where is this skill installed, and through which profile?\".",
                    opts: &[
                        Opt {
                            long: "profile",
                            short: None,
                            value: Some("NAME"),
                            help: "Show the repositories that have this profile enabled",
                        },
                        Opt {
                            long: "skill",
                            short: None,
                            value: Some("ID"),
                            help: "Show where this skill is installed and why",
                        },
                    ],
                    examples: &[
                        "beskar registry list",
                        "beskar registry list --profile coding",
                        "beskar registry list --skill playwright",
                    ],
                    json: true,
                    ..Spec::EMPTY
                },
                Spec {
                    name: "status",
                    about: "Show which repositories need updating",
                    details: "One line per repository: up to date, changes waiting, local changes that need a decision, missing folder, or broken. Changes nothing.",
                    json: true,
                    ..Spec::EMPTY
                },
                Spec {
                    name: "stats",
                    about: "Count repositories, profiles and skills",
                    details: "Also counts skills that no repository uses. Add --verbose to list them; they are candidates for cleaning up the library.",
                    opts: &[Opt {
                        long: "verbose",
                        short: Some('v'),
                        value: None,
                        help: "List the unused and unassigned skills by name",
                    }],
                    json: true,
                    ..Spec::EMPTY
                },
                Spec {
                    name: "update",
                    about: "Update every registered repository",
                    details: "Consults the registry and reconciles each repository with its enabled profiles, the way 'beskar repo update' does for one. \
Repositories that need nothing are skipped. A problem in one repository is reported and the others carry on.\n\n\
Run this after improving a skill in the library to propagate it everywhere.",
                    opts: &[ALL, DRY_RUN, ON_CONFLICT, VERBOSE_UPDATE],
                    examples: &[
                        "beskar registry update --all",
                        "beskar registry update --all --dry-run",
                    ],
                    json: true,
                    ..Spec::EMPTY
                },
                Spec {
                    name: "prune",
                    about: "Forget repositories whose folders no longer exist",
                    details: "Removes registry entries only. It never touches files.",
                    opts: &[DRY_RUN],
                    ..Spec::EMPTY
                },
            ],
            ..Spec::EMPTY
        },
        Spec {
            name: "skill",
            about: "Review and promote local changes to an installed skill",
            details: "Installed skills are copies, so a repository can drift from the library. These commands show the difference and let you send an improvement back.",
            subs: &[
                Spec {
                    name: "diff",
                    about: "Show how a repository's copy differs from the library's version",
                    details: "Prints a unified diff. Lines starting with - are in the library, lines starting with + are in the repository.",
                    args: &[SKILL_ARG],
                    opts: &[REPO],
                    examples: &["beskar skill diff code-review"],
                    ..Spec::EMPTY
                },
                Spec {
                    name: "promote",
                    about: "Copy a repository's version of a skill into the library",
                    details: "Replaces the library's version with the copy from the repository, so other repositories can pick it up with 'beskar registry update --all'. \
If the library changed since the copy was installed, promoting would discard those changes, so it needs --force.",
                    args: &[SKILL_ARG],
                    opts: &[
                        REPO,
                        YES,
                        Opt {
                            long: "force",
                            short: None,
                            value: None,
                            help: "Promote even if that discards changes made in the library",
                        },
                    ],
                    examples: &[
                        "beskar skill promote code-review",
                        "beskar skill promote code-review --repo ~/projects/api --yes",
                    ],
                    ..Spec::EMPTY
                },
            ],
            ..Spec::EMPTY
        },
        Spec {
            name: "status",
            about: "Shortcut: 'repo status', or 'registry status' with --all",
            args: &[REPO_PATH],
            opts: &[ALL, VERBOSE_STATUS],
            json: true,
            ..Spec::EMPTY
        },
        Spec {
            name: "update",
            about: "Shortcut: 'repo update', or 'registry update' with --all",
            details: UPDATE_DETAILS,
            args: &[REPO_PATH],
            opts: &[ALL, DRY_RUN, ON_CONFLICT, VERBOSE_UPDATE],
            examples: &["beskar update", "beskar update --all --dry-run"],
            json: true,
            ..Spec::EMPTY
        },
        Spec {
            name: "help",
            about: "Show help for a command, or a topic",
            details: "Topics:\n\n  format    the bsk file format used for settings, profiles and the registry",
            args: &[Arg {
                name: "TOPIC",
                help: "A command such as 'repo update', or the topic 'format'",
                required: false,
                many: true,
            }],
            examples: &["beskar help repo update", "beskar help format"],
            ..Spec::EMPTY
        },
    ],
    examples: &[
        "beskar init",
        "beskar library scan ~/my-skills",
        "beskar profile create coding git code-review testing",
        "cd ~/projects/app && beskar repo add .",
        "beskar repo enable coding",
        "beskar repo update",
    ],
    default_sub: None,
    json: false,
};

/// Finds a command by its names, for `beskar help <command...>`.
pub fn lookup(names: &[String]) -> Option<(Vec<&'static str>, &'static Spec)> {
    let mut spec = &ROOT;
    let mut path = Vec::new();
    for name in names {
        let sub = spec.subs.iter().find(|s| s.name == name)?;
        path.push(sub.name);
        spec = sub;
    }
    Some((path, spec))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn walk(
        spec: &'static Spec,
        path: &mut Vec<&'static str>,
        visit: &mut dyn FnMut(&[&'static str], &'static Spec),
    ) {
        visit(path, spec);
        for sub in spec.subs {
            path.push(sub.name);
            walk(sub, path, visit);
            path.pop();
        }
    }

    #[test]
    fn every_command_has_a_one_line_summary_without_trailing_punctuation_noise() {
        walk(&ROOT, &mut Vec::new(), &mut |path, spec| {
            assert!(!spec.about.is_empty(), "{path:?} needs an 'about'");
            assert!(
                !spec.about.contains('\n'),
                "{path:?}: 'about' must be one line"
            );
            assert!(
                !spec.about.ends_with('.'),
                "{path:?}: 'about' should not end with a period"
            );
        });
    }

    #[test]
    fn commands_with_subcommands_take_no_arguments_and_names_are_unique() {
        walk(&ROOT, &mut Vec::new(), &mut |path, spec| {
            if !spec.subs.is_empty() {
                assert!(spec.args.is_empty(), "{path:?} is a group");
                let mut names: Vec<&str> = spec.subs.iter().map(|s| s.name).collect();
                names.sort();
                let before = names.len();
                names.dedup();
                assert_eq!(before, names.len(), "duplicate subcommand under {path:?}");
            }
            if let Some(default) = spec.default_sub {
                assert!(
                    spec.subs.iter().any(|s| s.name == default),
                    "{path:?}: unknown default '{default}'"
                );
            }
        });
    }

    #[test]
    fn option_names_do_not_clash_with_each_other_or_the_global_options() {
        walk(&ROOT, &mut Vec::new(), &mut |path, spec| {
            let mut longs: Vec<&str> = spec.opts.iter().map(|o| o.long).collect();
            longs.extend(crate::args::GLOBAL_OPTS.iter().map(|o| o.long));
            if spec.json {
                longs.push("json");
            }
            let mut shorts: Vec<char> = spec.opts.iter().filter_map(|o| o.short).collect();
            shorts.extend(crate::args::GLOBAL_OPTS.iter().filter_map(|o| o.short));
            let (l, s) = (longs.len(), shorts.len());
            longs.sort();
            longs.dedup();
            shorts.sort();
            shorts.dedup();
            assert_eq!((longs.len(), shorts.len()), (l, s), "clash in {path:?}");
        });
    }

    #[test]
    fn required_arguments_come_before_optional_ones_and_only_the_last_repeats() {
        walk(&ROOT, &mut Vec::new(), &mut |path, spec| {
            let mut seen_optional = false;
            for (i, arg) in spec.args.iter().enumerate() {
                assert!(
                    !(arg.required && seen_optional),
                    "{path:?}: required after optional"
                );
                seen_optional |= !arg.required;
                assert!(
                    !arg.many || i == spec.args.len() - 1,
                    "{path:?}: only the last argument may repeat"
                );
            }
        });
    }

    #[test]
    fn examples_start_with_a_real_command() {
        walk(&ROOT, &mut Vec::new(), &mut |path, spec| {
            for example in spec.examples {
                let words: Vec<String> = example
                    .split(" && ")
                    .last()
                    .unwrap()
                    .split_whitespace()
                    .skip(1)
                    .map(String::from)
                    .collect();
                assert!(example.contains("beskar"), "{path:?}: {example}");
                if let Some(first) = words.first().filter(|w| !w.starts_with('-')) {
                    assert!(
                        ROOT.subs.iter().any(|s| s.name == first),
                        "{path:?}: example uses unknown command '{first}'"
                    );
                }
            }
        });
    }

    #[test]
    fn lookup_resolves_nested_commands() {
        let (path, spec) = lookup(&["repo".into(), "update".into()]).unwrap();
        assert_eq!(path, ["repo", "update"]);
        assert_eq!(spec.name, "update");
        assert!(lookup(&["nope".into()]).is_none());
        assert_eq!(lookup(&[]).unwrap().1.name, "beskar");
    }

    #[test]
    fn every_documented_command_from_the_brief_exists() {
        let expected = [
            "init",
            "doctor",
            "library init",
            "library add",
            "library scan",
            "library list",
            "library show",
            "library remove",
            "profile create",
            "profile delete",
            "profile list",
            "profile show",
            "profile add",
            "profile remove",
            "repo add",
            "repo remove",
            "repo list",
            "repo status",
            "repo enable",
            "repo disable",
            "repo toggle",
            "repo update",
            "registry list",
            "registry status",
            "registry stats",
            "registry update",
            "registry prune",
            "skill promote",
            "status",
            "update",
        ];
        for command in expected {
            let names: Vec<String> = command.split(' ').map(String::from).collect();
            assert!(
                lookup(&names).is_some(),
                "missing command: beskar {command}"
            );
        }
    }
}
