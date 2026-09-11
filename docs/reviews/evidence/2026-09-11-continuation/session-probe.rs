// Temporary example compiled against the normal library (NOT cfg(test)).
// Exercises real cross-process swap naming and writes in isolated XDG state.
use std::io::{self, Write};
#[allow(clippy::print_stdout)]
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let path = std::path::PathBuf::from(&args[1]);
    let body = &args[2];
    let mut editor = wordcartel::editor::Editor::new_from_text(body, Some(path.clone()), (80,24));
    editor.active_mut().document.version = 1;
    let header = wordcartel::swap::build_header(&editor, body, 1);
    let swap = wordcartel::swap::swap_path(Some(&path)).unwrap();
    wordcartel::swap::write_atomic(&swap, &wordcartel::swap::serialize(&header, body)).unwrap();
    println!("{}", swap.display());
    io::stdout().flush().unwrap();
    let mut command = String::new();
    io::stdin().read_line(&mut command).unwrap(); // remain live until controller finishes
}
