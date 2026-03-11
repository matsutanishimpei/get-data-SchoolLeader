use uiautomation::UIAutomation;
use uiautomation::types::TreeScope;
use uiautomation::types::UIProperty;
use uiautomation::variants::Variant;
use uiautomation::patterns::UISelectionItemPattern;
use uiautomation::patterns::UIValuePattern;
use uiautomation::UIElement;
use std::thread::sleep;
use std::time::Duration;
use std::fs::{self, OpenOptions};
use std::io::{Write, stdin};
use serde::Deserialize;

const UIA_LIST_CONTROL_TYPE: i32 = 50008;
const UIA_LIST_ITEM_CONTROL_TYPE: i32 = 50007;
const UIA_EDIT_CONTROL_TYPE: i32 = 50004;

#[derive(Debug, Deserialize)]
struct Config {
    window_title: String,
    #[serde(default = "default_wait_ms")]
    wait_ms: u64,
    phases: Vec<Phase>,
}

fn default_wait_ms() -> u64 {
    300
}

#[derive(Debug, Deserialize)]
struct Phase {
    name: String,
    fields: Vec<Field>,
}

#[derive(Debug, Deserialize)]
struct Field {
    access_name: String,
    csv_name: String,
    #[serde(default)]
    rightmost: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("--- Student List Clicker ---");
    println!("1: データ抽出モード (Accessから情報を取得して保存)");
    println!("2: 閲覧モード (保存済みデータを一覧表示)");
    println!("選択してください (1 or 2):");

    let mut choice = String::new();
    stdin().read_line(&mut choice)?;

    let mut wait_override = None;
    let args: Vec<String> = std::env::args().collect();
    for i in 0..args.len() {
        if (args[i] == "--wait-ms" || args[i] == "-w") && i + 1 < args.len() {
            if let Ok(ms) = args[i+1].parse::<u64>() {
                wait_override = Some(ms);
            }
        }
    }

    if choice.trim() == "2" {
        run_viewer()?;
    } else {
        if let Err(e) = run_collector(wait_override) {
            eprintln!("抽出実行中にエラーが発生しました: {:?}", e);
        }
    }

    Ok(())
}

fn run_collector(wait_ms_override: Option<u64>) -> uiautomation::Result<()> {
    // 設定ファイルの読み込み
    let config_content = fs::read_to_string("config.toml")
        .map_err(|e| uiautomation::Error::from(format!("Failed to read config.toml: {}", e)))?;
    let mut config: Config = toml::from_str(&config_content)
        .map_err(|e| uiautomation::Error::from(format!("Failed to parse config.toml: {}", e)))?;

    if let Some(ms) = wait_ms_override {
        config.wait_ms = ms;
    }

    let automation = UIAutomation::new()?;
    let root = automation.get_root_element()?;

    println!("Searching for main window containing '{}'...", config.window_title);
    let condition = automation.create_true_condition()?;
    let children = root.find_all(TreeScope::Children, &condition)?;
    
    let mut main_window = None;
    for child in children.iter() {
        if let Ok(name) = child.get_name() {
            if name.contains(&config.window_title) {
                main_window = Some(child.clone());
                break;
            }
        }
    }

    let main_window = match main_window {
        Some(el) => el,
        None => { println!("Main window not found."); return Ok(()); }
    };

    println!("Searching for '学生リスト'...");
    let name_only_cond = automation.create_property_condition(UIProperty::Name, Variant::from("学生リスト"), None)?;
    let mut list_element = main_window.find_first(TreeScope::Descendants, &name_only_cond).ok();
    if list_element.is_none() {
        let type_only_cond = automation.create_property_condition(UIProperty::ControlType, Variant::from(UIA_LIST_CONTROL_TYPE), None)?;
        list_element = main_window.find_first(TreeScope::Descendants, &type_only_cond).ok();
    }

    let list_element = match list_element {
        Some(el) => el,
        None => { println!("'学生リスト' was not found."); return Ok(()); }
    };

    let item_condition = automation.create_property_condition(UIProperty::ControlType, Variant::from(UIA_LIST_ITEM_CONTROL_TYPE), None)?;
    let items = list_element.find_all(TreeScope::Children, &item_condition)?;
    let detected_count = items.len();
    println!("Detected {} items in list.", detected_count);

    // --- 処理件数の指定 ---
    println!("抽出する最大人数を入力してください (例: 49):");
    let mut limit_str = String::new();
    let _ = stdin().read_line(&mut limit_str);
    let mut limit: usize = limit_str.trim().parse().unwrap_or(detected_count);
    if limit > detected_count { limit = detected_count; }
    println!("{} 人分を処理します。", limit);

    // データ格納用ベクタ (指定人数分)
    let mut all_student_data: Vec<Vec<String>> = vec![Vec::new(); limit];

    for phase in &config.phases {
        println!("\n【STEP】アプリ上で「{}」のタブを手動で選択してください。", phase.name);
        println!("準備ができたら、このターミナルで Enter キーを押してください...");
        let mut buffer = String::new();
        let _ = stdin().read_line(&mut buffer);

        println!("--- 自動抽出を開始します ({}人まで) ---", limit);
        for i in 0..limit {
            let item = &items[i];
            
            // 行を選択
            let _ = item.set_focus();
            let mut selected = false;
            if let Ok(p) = item.get_pattern::<UISelectionItemPattern>() {
                if p.select().is_ok() { selected = true; }
            }
            if !selected { let _ = item.click(); }
            sleep(Duration::from_millis(config.wait_ms)); // Access側の表示更新待ち

            for field in &phase.fields {
                let val = if field.rightmost {
                    get_field_value_rightmost(&automation, &main_window, &field.access_name).unwrap_or_default()
                } else {
                    get_field_value(&automation, &main_window, &field.access_name).unwrap_or_default()
                };

                all_student_data[i].push(val);
            }
            
            if (i+1) % 10 == 0 || i == limit - 1 {
                println!("  Progress: {}/{}", i + 1, limit);
            }
        }
    }

    // --- すべてを合体させて保存 ---
    println!("\nFinalizing and saving to student_data.txt...");
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open("student_data.txt")
        .map_err(|e| uiautomation::Error::from(e.to_string()))?;

    // ヘッダーを動的に生成して書き込む
    let mut header_parts = Vec::new();
    for phase in &config.phases {
        for field in &phase.fields {
            header_parts.push(field.csv_name.as_str());
        }
    }
    let header = header_parts.join(",");
    let _ = writeln!(file, "{}", header);

    for row in all_student_data {
        let line = row.join(",");
        let _ = writeln!(file, "{}", line);
    }

    println!("All done! student_data.txt has been updated.");
    Ok(())
}

fn run_viewer() -> Result<(), Box<dyn std::error::Error>> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 800.0])
            .with_title("Student Data Viewer"),
        ..Default::default()
    };
    
    eframe::run_native(
        "Student Data Viewer",
        options,
        Box::new(|_cc| Ok(Box::new(ViewerApp::new())))
    ).map_err(|e| format!("GUI Error: {}", e).into())
}

struct ViewerApp {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
    error_msg: Option<String>,
}

impl ViewerApp {
    fn new() -> Self {
        match fs::read_to_string("student_data.txt") {
            Ok(content) => {
                let mut lines = content.lines();
                let headers = lines.next()
                    .unwrap_or_default()
                    .split(',')
                    .map(|s| s.to_string())
                    .collect();
                let rows = lines.map(|line| {
                    line.split(',').map(|s| s.to_string()).collect()
                }).collect();
                Self { headers, rows, error_msg: None }
            }
            Err(e) => Self {
                headers: Vec::new(),
                rows: Vec::new(),
                error_msg: Some(format!("Failed to load student_data.txt: {}", e)),
            }
        }
    }
}

impl eframe::App for ViewerApp {
    fn update(&mut self, ctx: &eframe::egui::Context, _frame: &mut eframe::Frame) {
        eframe::egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Student Data Viewer");
            
            if let Some(msg) = &self.error_msg {
                ui.colored_label(eframe::egui::Color32::RED, msg);
                return;
            }

            if self.rows.is_empty() {
                ui.label("表示するデータがありません。");
                return;
            }

            ui.add_space(10.0);

            
            use egui_extras::{TableBuilder, Column};
            TableBuilder::new(ui)
                .striped(true)
                .resizable(true)
                .cell_layout(eframe::egui::Layout::left_to_right(eframe::egui::Align::Center))
                .columns(Column::initial(100.0).at_least(50.0), self.headers.len())
                .min_scrolled_height(0.0)
                .header(25.0, |mut header| {
                    for h in &self.headers {
                        header.col(|ui| {
                            ui.strong(h);
                        });
                    }
                })
                .body(|body| {
                    body.rows(20.0, self.rows.len(), |mut row| {
                        let row_index = row.index();
                        let data_row = &self.rows[row_index];
                        for val in data_row {
                            row.col(|ui| {
                                ui.label(val);
                            });
                        }
                    });
                });
        });
    }
}

fn get_field_value(automation: &UIAutomation, root: &UIElement, name: &str) -> Option<String> {
    let name_cond = automation.create_property_condition(UIProperty::Name, Variant::from(name), None).ok()?;
    let edit_cond = automation.create_property_condition(UIProperty::ControlType, Variant::from(UIA_EDIT_CONTROL_TYPE), None).ok()?;
    let strict_cond = automation.create_and_condition(name_cond.clone(), edit_cond).ok()?;

    if let Ok(element) = root.find_first(TreeScope::Descendants, &strict_cond) {
        if let Ok(pattern) = element.get_pattern::<UIValuePattern>() {
            if let Ok(val) = pattern.get_value() {
                if !val.is_empty() { return Some(val.replace(",", " ")); }
            }
        }
    }
    if let Ok(element) = root.find_first(TreeScope::Descendants, &name_cond) {
        if let Ok(pattern) = element.get_pattern::<UIValuePattern>() {
            if let Ok(val) = pattern.get_value() { 
                if !val.is_empty() { return Some(val.replace(",", " ")); }
            }
        }
        if let Ok(val) = element.get_name() { if val != name { return Some(val.replace(",", " ")); } }
    }
    None
}

fn get_field_value_rightmost(automation: &UIAutomation, root: &UIElement, name: &str) -> Option<String> {
    let name_cond = automation.create_property_condition(UIProperty::Name, Variant::from(name), None).ok()?;
    if let Ok(elements) = root.find_all(TreeScope::Descendants, &name_cond) {
        let mut target_element = None;
        let mut max_left: i32 = -2147483648;
        for el in elements.iter() {
            if let Ok(rect) = el.get_bounding_rectangle() {
                let left = rect.get_left();
                if left > max_left { max_left = left; target_element = Some(el.clone()); }
            }
        }
        if let Some(element) = target_element {
            if let Ok(pattern) = element.get_pattern::<UIValuePattern>() {
                if let Ok(val) = pattern.get_value() { return Some(val.replace(",", " ")); }
            }
            if let Ok(val) = element.get_name() { if val != name { return Some(val.replace(",", " ")); } }
        }
    }
    None
}
