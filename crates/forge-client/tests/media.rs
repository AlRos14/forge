use std::path::PathBuf;

use clap::{Parser, Subcommand};
use forge_client::task::{MediaCmd, TaskArgs, TaskCmd};

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Task(TaskArgs),
}

#[test]
fn parses_task_media_upload() {
    let command = parse_task([
        "forge-ctl",
        "task",
        "media",
        "upload",
        "--task-id",
        "abc",
        "--file",
        "/tmp/shot.png",
    ]);

    let TaskCmd::Media(args) = command else {
        panic!("expected media command");
    };
    let MediaCmd::Upload {
        task_id,
        file,
        author_name,
    } = args.cmd
    else {
        panic!("expected media upload command");
    };

    assert_eq!(task_id, "abc");
    assert_eq!(file, PathBuf::from("/tmp/shot.png"));
    assert_eq!(author_name, None);
}

#[test]
fn parses_task_media_upload_with_author_name() {
    let command = parse_task([
        "forge-ctl",
        "task",
        "media",
        "upload",
        "--task-id",
        "abc",
        "--file",
        "/tmp/shot.png",
        "--author-name",
        "Agent",
    ]);

    let TaskCmd::Media(args) = command else {
        panic!("expected media command");
    };
    let MediaCmd::Upload {
        task_id,
        file,
        author_name,
    } = args.cmd
    else {
        panic!("expected media upload command");
    };

    assert_eq!(task_id, "abc");
    assert_eq!(file, PathBuf::from("/tmp/shot.png"));
    assert_eq!(author_name.as_deref(), Some("Agent"));
}

#[test]
fn parses_task_media_comment() {
    let command = parse_task([
        "forge-ctl",
        "task",
        "media",
        "comment",
        "--task-id",
        "abc",
        "--content",
        "looks good",
    ]);

    let TaskCmd::Media(args) = command else {
        panic!("expected media command");
    };
    let MediaCmd::Comment {
        task_id,
        content,
        author_name,
        media_url,
    } = args.cmd
    else {
        panic!("expected media comment command");
    };

    assert_eq!(task_id, "abc");
    assert_eq!(content, "looks good");
    assert_eq!(author_name, None);
    assert!(media_url.is_empty());
}

#[test]
fn parses_task_media_comment_with_two_media_urls() {
    let command = parse_task([
        "forge-ctl",
        "task",
        "media",
        "comment",
        "--task-id",
        "abc",
        "--content",
        "looks good",
        "--media-url",
        "/api/v1/media/one.png",
        "--media-url",
        "/api/v1/media/two.mp4",
    ]);

    let TaskCmd::Media(args) = command else {
        panic!("expected media command");
    };
    let MediaCmd::Comment {
        task_id,
        content,
        author_name,
        media_url,
    } = args.cmd
    else {
        panic!("expected media comment command");
    };

    assert_eq!(task_id, "abc");
    assert_eq!(content, "looks good");
    assert_eq!(author_name, None);
    assert_eq!(
        media_url,
        vec![
            "/api/v1/media/one.png".to_owned(),
            "/api/v1/media/two.mp4".to_owned()
        ]
    );
}

fn parse_task<const N: usize>(args: [&str; N]) -> TaskCmd {
    let parsed = Cli::try_parse_from(args).expect("task media command parses");
    let Commands::Task(task) = parsed.command;
    task.cmd
}
