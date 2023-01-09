use crate::get_files_dir;
use fpm_importer::errors::Result;
use fpm_importer::MDBook;

pub fn execute(dir_name: &str) -> Result<()> {
    let book_dir = get_files_dir(dir_name);
    dbg!(&book_dir);
    let book = MDBook::load(&book_dir)?;
    book.build()?;
    Ok(())
}
