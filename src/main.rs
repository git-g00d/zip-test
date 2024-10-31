use std::{io::{Read, Write}, path::PathBuf};
use anyhow::{bail, Error, Context};
use env_logger;
use log::info;
use clap::{Parser, Subcommand};


fn main() -> Result<(), Error> {
    env_logger::init();

    // Parse the command line args
    let args = Args::parse();
    info!("Args: {args:?}");

    match args.commands {
        Commands::Zip { src, dst, method, mode, chunk, password } => create_archive(src, dst, method, mode, chunk, password),
        Commands::Unzip { archive, output_dir } => extract_archive(archive, output_dir),
    }
}

#[derive(Subcommand, Debug, Clone)]
#[clap(rename_all = "kebab_case")]
pub enum Commands {
    Zip {
        #[arg(short = 's', long = "source")]
        src: PathBuf,
        #[arg(short = 'd', long = "dest")]
        dst: PathBuf,
        #[arg(short = 'm', long = "method")]
        method: u16,
        #[arg(short = 'M', long = "mode")]
        mode: Option<u32>,
        #[arg(short = 'c', long = "chunk")]
        chunk: usize,
        #[arg(short = 'p', long = "password")]
        password: Option<String>
    },
    Unzip {
        /// Show supported features as strings and exit
        #[arg(short = 'a', long = "archive")]
        archive: PathBuf,
        #[arg(short = 'o', long = "output")]
        output_dir: Option<PathBuf>
    }
}

#[derive(Parser, Debug, Clone)]
#[command(author, about, long_about = None)]
#[group(multiple=false)]
pub struct Args {
    /// Subcommands for the Agent
    #[command(subcommand)]
    pub commands: Commands,
}

pub fn extract_archive(archive: PathBuf, output_dir: Option<PathBuf>) -> Result<(), Error> {
    let output_dir = if let Some(d) = output_dir {
        Some(d)
    } else {
        None
    };

    let file = std::fs::File::open(archive)?;

    let mut archive = zip::ZipArchive::new(file)?;

    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let outpath = match file.enclosed_name() {
            Some(path) => {
                if let Some(d) = &output_dir {
                    d.join(path)
                } else {
                    path
                }
            },
            None => continue,
        };

        if file.is_dir() {
            std::fs::create_dir_all(&outpath)?;
        } else {
            if let Some(p) = outpath.parent() {
                if !p.exists() {
                    std::fs::create_dir_all(p)?;
                }
            }
            let mut outfile = std::fs::File::create(&outpath)?;
            std::io::copy(&mut file, &mut outfile)?;
        }

        // Get and Set permissions
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            if let Some(mode) = file.unix_mode() {
                std::fs::set_permissions(&outpath, std::fs::Permissions::from_mode(mode))?;
            }
        }
    }

    Ok(())
}

pub fn create_archive(src: PathBuf, dst: PathBuf, method: u16, mode: Option<u32>, chunk: usize, password: Option<String>) -> Result<(), Error> {
    if !src.is_dir() {
        compress_file(&src, dst, method, mode, chunk, password)?
    } else {
        let method = into_comp_method(method);

        let walkdir = walkdir::WalkDir::new(&src);
    
        let file = std::fs::File::create(dst)?;
        let mut options = zip::write::SimpleFileOptions::default().compression_method(method);
        let mut pw_str = String::new();
        if let Some(pw) = password {
            pw_str.push_str(&pw);
            options = options
                    .compression_method(method)
                    .with_aes_encryption(zip::AesMode::Aes256, &pw_str)
        }
        let mut zip = zip::ZipWriter::new(file);
        
        if let Some(m) = mode {
            options = options.unix_permissions(m);
        }
    
        let mut buf = vec![0u8; chunk];
        for entry in walkdir.into_iter() {
            let path = match &entry {
                Ok(e) => e.path(),
                Err(err) => bail!("Failed to open file: {err}"),
            };
            let name = path.strip_prefix(&src)?;
            let path_as_string = name
                .to_str()
                .map(str::to_owned)
                .with_context(|| format!("{name:?} Is a Non UTF-8 Path"))?;
    
            // Write file or directory explicitly
            // Some unzip tools unzip files with directory paths correctly, some do not!
            if path.is_file() {
                zip.start_file(path_as_string, options)?;
                let mut f = std::fs::File::open(path)?;

                loop {
                    let r = f.read(&mut buf)?;
                    zip.write_all(&buf[0..r])?;
                    if r == 0 {
                        break;
                    }
                    buf.clear();
                    buf.resize(chunk, 0u8);
                }
            } else if !name.as_os_str().is_empty() {
                zip.add_directory(path_as_string, options)?;
            }
        }
        zip.finish()?;
    }

    Ok(())
}

fn compress_file(src: &std::path::PathBuf, dst: PathBuf, method: u16, mode: Option<u32>, chunk: usize, password: Option<String>) -> Result<(), Error> {
    let file = std::fs::File::create(dst)?;

    let mut zip = zip::ZipWriter::new(file);

    let mut options = zip::write::SimpleFileOptions::default()
        .compression_method(into_comp_method(method));
    if let Some(m) = mode {
        options = options.unix_permissions(m);
    }

    let mut data = std::fs::File::open(src)?;

    let src_file = match src.to_str() {
        Some(s) => s,
        None => bail!("Invalid UTF-8: {src:?}"),
    };

    let mut buf = vec![0u8; chunk];
    if let Some(pw) = password {
        zip.start_file(
            src_file,
            options
                .compression_method(into_comp_method(method))
                .with_aes_encryption(zip::AesMode::Aes256, &pw),
        )?;
    } else {
        zip.start_file(
            src_file,
            options
                .compression_method(into_comp_method(method))
        )?;
    };

    loop {
        let r = data.read(&mut buf)?;
        zip.write_all(&buf[0..r])?;
        if r == 0 {
            break;
        }
        buf.clear();
        buf.resize(chunk, 0u8);
    }

    zip.finish()?;

    Ok(())
}

fn into_comp_method(value: u16) -> zip::CompressionMethod {
    match value {
        0 => zip::CompressionMethod::Stored,
        1 => zip::CompressionMethod::Deflated,
        2 => zip::CompressionMethod::Deflate64,
        3 => zip::CompressionMethod::Bzip2,
        4 => zip::CompressionMethod::Aes,
        5 => zip::CompressionMethod::Zstd,
        6 => zip::CompressionMethod::Lzma,
        _i => zip::CompressionMethod::Deflated,
    }
}