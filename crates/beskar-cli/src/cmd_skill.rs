//! `beskar skill ...`: comparing and promoting workspace copies.

use beskar_core::{Result, SkillId, diff, err, reconcile};

use crate::args::Args;
use crate::cmd_library::hint_propagate;
use crate::cmd_repo::target;
use crate::ui::show;
use crate::update::render_diff;
use crate::{Ctx, Exit};

pub fn diff(ctx: &Ctx, args: &Args) -> Result<Exit> {
    let b = ctx.open()?;
    let path = target(&b, args)?;
    let repo = b.registry.get(&path).expect("target is registered");
    let id = SkillId::new(&args.positional[0])?;
    let local = repo.skills_path(&b.config.skills_dir).join(id.as_str());
    if !local.is_dir() {
        return Err(err!("`{id}` is not present in {}", show(&repo.path)));
    }
    let library = b.library.skill_path(&id);
    if !library.is_dir() {
        println!("`{id}` is not in the library; everything in the workspace copy is new.");
    }
    println!("--- library    {}", show(&library));
    println!("+++ workspace  {}\n", show(&local));
    print!("{}", render_diff(&ctx.ui, &library, &local));
    Ok(Exit::Ok)
}

pub fn promote(ctx: &Ctx, args: &Args) -> Result<Exit> {
    let mut b = ctx.open()?;
    let path = target(&b, args)?;
    let id = SkillId::new(&args.positional[0])?;
    let repo = b.registry.get(&path).expect("target is registered");
    let local = repo.skills_path(&b.config.skills_dir).join(id.as_str());
    let changes = diff::diff_dirs(&b.library.skill_path(&id), &local)?;
    if local.is_dir() && !changes.is_empty() {
        println!("Promoting `{id}` from {} changes:", show(&repo.path));
        for c in &changes {
            let mark = match c {
                diff::Change::Added(_) => '+',
                diff::Change::Removed(_) => '-',
                diff::Change::Modified(_) => '~',
            };
            println!("  {mark} {}", c.path());
        }
        println!("{}", ctx.ui.dim(&format!("(full diff: `beskar skill diff {id}`)")));
        if !args.has("yes") {
            if !ctx.ui.interactive() {
                println!("Not promoting without confirmation: re-run with --yes.");
                return Ok(Exit::Attention);
            }
            if !ctx.ui.confirm("Promote to the library?", false) {
                println!("Nothing changed.");
                return Ok(Exit::Ok);
            }
        }
    }
    let repo = b.registry.get_mut(&path).expect("target is registered");
    let promoted = reconcile::promote(&b.library, repo, &b.config.skills_dir, &id, args.has("force"))?;
    b.registry.save()?;
    if promoted {
        println!("Promoted `{id}` to the library.");
        hint_propagate(ctx, &b, &id)?;
    } else {
        println!("`{id}` is identical to the library; nothing to promote.");
    }
    Ok(Exit::Ok)
}
