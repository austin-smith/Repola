use tauri::menu::{Menu, MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::{AppHandle, Runtime};

pub fn application_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let settings = MenuItemBuilder::with_id("settings", "Settings…")
        .accelerator("CmdOrCtrl+,")
        .build(app)?;
    let add_repository = MenuItemBuilder::with_id("add-repository", "Add Repository…")
        .accelerator("CmdOrCtrl+O")
        .build(app)?;
    let palette = MenuItemBuilder::with_id("command-palette", "Command Palette…")
        .accelerator("CmdOrCtrl+K")
        .build(app)?;
    let refresh = MenuItemBuilder::with_id("refresh", "Refresh")
        .accelerator("CmdOrCtrl+R")
        .build(app)?;
    let changes = MenuItemBuilder::with_id("view-changes", "Changes")
        .accelerator("CmdOrCtrl+1")
        .build(app)?;
    let history = MenuItemBuilder::with_id("view-history", "History")
        .accelerator("CmdOrCtrl+2")
        .build(app)?;
    let worktrees = MenuItemBuilder::with_id("view-worktrees", "Worktrees")
        .accelerator("CmdOrCtrl+3")
        .build(app)?;
    let select_all =
        MenuItemBuilder::with_id("changes-select-all", "Select All Files").build(app)?;

    let name = app.config().product_name.as_deref().unwrap_or("Repola");
    let application = SubmenuBuilder::new(app, name)
        .about(None)
        .separator()
        .item(&settings)
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .quit()
        .build()?;
    let file = SubmenuBuilder::new(app, "File")
        .item(&add_repository)
        .separator()
        .close_window()
        .build()?;
    let edit = SubmenuBuilder::new(app, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .item(&select_all)
        .build()?;
    let repository = SubmenuBuilder::new(app, "Repository")
        .item(&refresh)
        .build()?;
    let view = SubmenuBuilder::new(app, "View")
        .item(&palette)
        .separator()
        .item(&changes)
        .item(&history)
        .item(&worktrees)
        .separator()
        .fullscreen()
        .build()?;
    let window = SubmenuBuilder::new(app, "Window")
        .minimize()
        .maximize()
        .close_window()
        .build()?;
    MenuBuilder::new(app)
        .items(&[&application, &file, &edit, &repository, &view, &window])
        .build()
}
