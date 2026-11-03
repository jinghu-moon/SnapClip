//! Production pin views with local fixtures. Copy feedback is simulated;
//! this example never reads or writes the system clipboard.
use gpui_kit::{
    component::{ActiveTheme, Theme, ThemeMode},
    *,
};
use snapclip_pin::{
    DocumentMode, FileKind, PinCommand, PinContent, PinDocument, PinFile, PinImage, PinManager,
    PinPlacement, PinSnapshot, PinTable,
};
use std::sync::Arc;

struct Preview {
    _owner: Entity<PinManager>,
}
impl Global for Preview {}

fn sample(mode: &str) -> PinSnapshot {
    let content = match mode {
        "text" => PinContent::Document(PinDocument::new(
            "对照阅读\n\n中文、English、emoji 😀。\n\n  空格与\tTab 应保留；正文可以选择和复制。\r\n".into(),
            DocumentMode::Text,
        )),
        "code" => PinContent::Document(PinDocument::new(
            "fn main() {\n    let title = \"剪贴板贴图\";\n    println!(\"{title}\");\n}\n".into(),
            DocumentMode::Code,
        )),
        "table" => PinContent::Document(PinDocument::new(
            "项目\t数量\t占比\n图片\t123\t50%\n文档\t42\t30%\n文件\t\t20%".into(),
            DocumentMode::Table,
        ).with_table(PinTable::new(vec![
            vec!["项目".into(), "数量".into(), "占比".into()],
            vec!["图片".into(), "123".into(), "50%".into()],
            vec!["文档".into(), "42".into(), "30%".into()],
            vec!["文件".into(), String::new(), "20%".into()],
        ], true).unwrap())),
        "link" => PinContent::Document(PinDocument::new(
            "https://example.com/design?view=clipboard-pin".into(), DocumentMode::Link,
        )),
        "path" => PinContent::Document(PinDocument::new(
            r"C:\Samples\项目交付\设计规范.pdf".into(), DocumentMode::Path,
        )),
        "files" => PinContent::Files(Arc::new(vec![
            PinFile::new(r"C:\Samples\产品设计规范.pdf".into(), "产品设计规范.pdf".into(), FileKind::Document).with_size(1024 * 230),
            PinFile::new(r"C:\Samples\设计素材".into(), "设计素材".into(), FileKind::Folder),
            PinFile::new(r"C:\Samples\archive.zip".into(), "archive.zip".into(), FileKind::Archive).unavailable("文件已移动或删除（预览样本）".into()),
        ])),
        "image" => {
            let (w, h) = (480, 320);
            let rgba = (0..h).flat_map(|y| (0..w).flat_map(move |x| [
                (x * 255 / w) as u8, (y * 255 / h) as u8, 160, if x < 80 { 96 } else { 255 },
            ])).collect();
            PinContent::Image(PinImage::new(w, h, rgba).unwrap())
        }
        "rich" => PinContent::Document(PinDocument::new(
            "对照阅读\n\n原生文字可以选择，滚动与窗口移动各有入口。\n\n中文、English、emoji 😀 都应正常显示。\n\n快捷键：Ctrl+A 全选、Ctrl+C 复制选区、Esc 关闭。".into(),
            DocumentMode::Rich,
        ).with_html("<h3>对照阅读</h3><p>原生文字可以<strong>选择</strong>，滚动与窗口移动各有入口。</p><p>中文、English、emoji 😀 都应正常显示。</p><ul><li>Ctrl+A 全选</li><li>Ctrl+C 复制选区</li><li>Esc 关闭</li></ul>".into())),
        _ => panic!("unknown sample: {mode}; choose text, rich, table, code, link, path, files or image"),
    };
    PinSnapshot::new(content)
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "rich".into());
    let snapshot = Arc::new(sample(&mode));
    eprintln!(
        "[pin-preview] {mode}: 复制仅模拟成功反馈，不修改系统剪贴板；打开命令仍由生产代码执行。"
    );
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            if std::env::var_os("SNAPCLIP_PIN_DARK").is_some() {
                Theme::change(ThemeMode::Dark, None, cx);
            }
            let dark = cx.theme().mode == ThemeMode::Dark;
            yue_component::init(
                cx,
                yue_component::YueTheme::new(
                    if dark {
                        yue_component::ThemeMode::Dark
                    } else {
                        yue_component::ThemeMode::Light
                    },
                    yue_component::Accent::Azure,
                ),
            )
            .unwrap();
            snapclip_pin::init(cx);
            let (manager, commands) = PinManager::new().unwrap();
            let owner = cx.new(|_| manager);
            PinManager::open(snapshot, PinPlacement::Pointer((200, 200)), &owner, cx).unwrap();
            let copy_owner = owner.downgrade();
            cx.spawn(async move |cx| {
                while let Ok(PinCommand::Copy { id, .. }) = commands.recv().await {
                    if copy_owner
                        .update(cx, |manager, cx| manager.report_copy(id, Ok(()), cx))
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();
            cx.set_global(Preview { _owner: owner });
        });
}
