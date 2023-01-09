
use std::env;
use std::path::PathBuf;
mod cmd;

fn main() {
    
    cmd::build::execute("ftd");

}

fn get_files_dir(dir_name: &str) -> PathBuf {
        // Check if path is relative from current dir, or absolute...
        let mut path=PathBuf::new();
        path.push(dir_name);
        dbg!(&path);
        if path.is_relative() {
            env::current_dir().unwrap().join(path)
        } else {
            path.to_path_buf()
        }
   
}
