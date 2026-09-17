//! Nondestructive, bounded recovery selection overlay. No bodies, leases or filesystem IO.
use crate::app::{Handled, Msg};
use crate::editor::Editor;
use crate::recovery_discovery::{Candidate, CandidateTime, SelectionToken};
use crate::registry::Ctx;
use crossterm::event::{Event, KeyCode, KeyEventKind, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

#[derive(Debug, PartialEq, Eq)]
enum Phase { Loading, Selecting, Opening }
/// Picker holds only bounded candidate metadata; preparation owns selected bodies on worker.
#[derive(Debug)]
pub(crate) struct RecoveryPicker {
    rows: Vec<Candidate>,
    reports: Vec<Option<String>>,
    selected: Vec<bool>,
    cursor: usize,
    top: usize,
    phase: Phase,
    message: String,
    path_scroll: usize,
}
impl RecoveryPicker {
    pub(crate) fn ready(rows: Vec<Candidate>) -> Self {
        let selected = vec![false;rows.len()]; let reports=vec![None;rows.len()];
        let message = if rows.is_empty() { "No recovery files" }
            else { "Space: select   Enter: open selected   Esc: dismiss" }.into();
        Self {rows,reports,selected,cursor:0,top:0,phase:Phase::Selecting,message,path_scroll:0}
    }
    pub(crate) fn loading() -> Self {
        let mut p = Self::ready(Vec::new());
        p.phase = Phase::Loading;
        p.message = "Scanning recovery files…".into();
        p
    }
    pub(crate) fn error(message: String) -> Self {
        let mut p = Self::ready(Vec::new());
        p.message = message;
        p
    }
    pub(crate) fn is_loading(&self) -> bool { self.phase == Phase::Loading }
}
/// Every dismissal route uses the same exact-token suppression and cancellation rules.
pub(crate) fn close(editor: &mut Editor) {
    let Some(p) = editor.recovery_picker.take() else { return; };
    for row in &p.rows { crate::recovery_flow::discovery::dismiss(editor, row); }
    crate::recovery_flow::discovery::cancel_scans(editor);
    if p.phase == Phase::Opening { crate::recovery_flow::cancel_batch(editor); }
}
fn accept(ctx: &mut Ctx) {
    let Some(p) = ctx.editor.recovery_picker.as_mut() else { return; };
    if p.phase != Phase::Selecting { return; }
    let selected: Vec<_> = p.rows.iter().zip(&p.selected)
        .filter(|(r, s)| **s && r.unavailable.is_none() && r.token.is_some())
        .map(|(r, _)| r.clone()).collect();
    if selected.is_empty() { p.message="Select a recovery file".into(); return; }
    let unselected: Vec<_> = p.rows.iter().zip(&p.selected)
        .filter(|(_, chosen)| !**chosen).map(|(row, _)| row.clone()).collect();
    p.phase=Phase::Opening; p.message="Opening selected recovery files… Esc: cancel remaining".into();
    for row in &unselected { crate::recovery_flow::discovery::dismiss(ctx.editor, row); }
    crate::recovery_flow::begin_selected(ctx,selected);
    batch_boundary(ctx.editor);
}
/// Complete only this UI's batch; terminal failed rows remain available for explicit retry.
pub(crate) fn batch_boundary(editor: &mut Editor) {
    if crate::recovery_flow::imports_busy(editor) { return; }
    if let Some(p) = editor.recovery_picker.as_mut().filter(|p| p.phase == Phase::Opening) {
        p.phase=Phase::Selecting; p.selected.fill(false);
        if p.message.starts_with("Opening selected") {
            p.message = "Recovery opening complete. Esc: close; select a file to retry or focus".into();
        }
    }
}
/// Report a prepare failure or changed generation without silently selecting replacement text.
pub(crate) fn row_result(editor: &mut Editor, token: Option<&SelectionToken>,
    changed: Option<Candidate>, message: &str) {
    let Some(p) = editor.recovery_picker.as_mut().filter(|p| p.phase == Phase::Opening) else { return; };
    p.message=message.into();
    if let Some(index) = p.rows.iter().position(|r| r.token.as_ref() == token) {
        if let Some(row) = changed { p.rows[index]=row; }
        p.selected[index]=false; p.reports[index]=Some(message.into());
    }
}
/// One geometry drives both clipped painting and mouse hit testing, including tiny terminals.
fn geometry(area: Rect) -> (Rect, Rect, Rect, Rect) {
    let width=area.width.min(88); let height=area.height.min(22);
    let outer=Rect::new(area.x+(area.width-width)/2,area.y+(area.height-height)/2,width,height);
    let inner=outer.inner(ratatui::layout::Margin::new(1,1));
    let preview_h=if inner.height >= 10 { 7 } else if inner.height >= 6 { 3 } else { 0 };
    let rows_h=inner.height.saturating_sub(preview_h+1);
    (outer,Rect::new(inner.x,inner.y,inner.width,rows_h),
        Rect::new(inner.x,inner.y+rows_h,inner.width,preview_h),
        Rect::new(inner.x,inner.y+rows_h+preview_h,inner.width,inner.height.saturating_sub(rows_h+preview_h)))
}
fn window(p: &mut RecoveryPicker, height: u16) {
    if height == 0 { return; }
    if p.cursor < p.top { p.top=p.cursor; }
    if p.cursor >= p.top+height as usize { p.top=p.cursor+1-height as usize; }
}
fn navigate(editor: &mut Editor, down: bool) {
    let area=editor.active().view.area;
    if let Some(p) = editor.recovery_picker.as_mut() {
        p.cursor = if down { (p.cursor + 1).min(p.rows.len().saturating_sub(1)) }
            else { p.cursor.saturating_sub(1) };
        p.path_scroll=0;
        window(p,geometry(Rect::new(0,0,area.0,area.1)).1.height);
    }
}
fn toggle(editor: &mut Editor) {
    if let Some(p) = editor.recovery_picker.as_mut().filter(|p| p.phase == Phase::Selecting) {
        if p.rows.get(p.cursor).is_some_and(|r| r.unavailable.is_none() && r.token.is_some()) {
            p.selected[p.cursor] = !p.selected[p.cursor];
        }
    }
}
/// Consume editing input but leave JobDone and other completion messages on the normal route.
pub(crate) fn intercept(msg: Msg, editor: &mut Editor, dc: &crate::overlays::DispatchCtx) -> Handled {
    if editor.recovery_picker.is_none() { return Handled::Pass(msg); }
    match &msg {
        Msg::Input(Event::Key(k)) => {
            if k.kind == KeyEventKind::Press {
                match k.code {
                    KeyCode::Esc => close(editor),
                    KeyCode::Up => navigate(editor, false),
                    KeyCode::Down => navigate(editor, true),
                    KeyCode::Char(' ') => toggle(editor),
                    KeyCode::Left => scroll_path(editor,false), KeyCode::Right => scroll_path(editor,true),
                    KeyCode::Enter => accept(&mut Ctx { editor, executor: dc.ex, clock: dc.clock,
                        msg_tx: dc.msg_tx.clone(), fs: dc.fs.clone() }),
                    _ => (),
                }
            }
        }
        Msg::Input(Event::Paste(_)) | Msg::ClipboardPaste { .. } => (),
        _ => return Handled::Pass(msg),
    }
    Handled::Done(crate::app::fold_and_continue(editor,dc.ex,dc.clock,dc.msg_tx,dc.fs))
}
/// Mouse selection uses the exact viewport used for paint. Click-away is nondestructive.
pub(crate) fn mouse(editor: &mut Editor, event: MouseEvent, area: Rect, _: &crate::overlays::DispatchCtx) {
    match event.kind {
        MouseEventKind::ScrollUp => navigate(editor,false), MouseEventKind::ScrollDown => navigate(editor,true),
        MouseEventKind::Down(MouseButton::Left) => {
            let (outer,rows,_,_)=geometry(area); let point=(event.column,event.row).into();
            if !outer.contains(point) { close(editor); return; }
            if rows.contains(point) {
                if let Some(p)=editor.recovery_picker.as_mut() {
                    window(p,rows.height);
                    let index=p.top+(event.row-rows.y) as usize;
                    if index >= p.rows.len() { return; } p.cursor=index; p.path_scroll=0;
                }
                toggle(editor);
            }
        }
        _ => (),
    }
}
// Gregorian civil date from a Unix day count; bounded metadata timestamps end in year 9999.
fn utc_time(seconds: u64) -> String {
    let days=seconds/86400+719468; let era=days/146097; let doe=days%146097;
    let yoe=(doe-doe/1460+doe/36524-doe/146096)/365;
    let mut year=yoe+era*400; let doy=doe-(365*yoe+yoe/4-yoe/100); let mp=(5*doy+2)/153;
    let day=doy-(153*mp+2)/5+1; let month=if mp<10 {mp+3} else {mp-9};
    if month<=2 { year+=1; }
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",seconds%86400/3600,seconds%3600/60)
}
fn display_paths(row: &Candidate) -> (String, String) {
    let tagged=row.association.as_ref().or(row.provenance.as_ref());
    if let Some(tagged)=tagged.filter(|p| p.local_path().is_none()) {
        use crate::recovery_store::TaggedPath;
        let filename=match tagged {
            TaggedPath::Windows(units) => TaggedPath::Windows(
                units.rsplit(|u| *u == 47 || *u == 92).next().unwrap_or(units).to_vec()),
            TaggedPath::Unix(units) => TaggedPath::Unix(units.rsplit(|u| *u==b'/').next().unwrap_or(units).to_vec()),
        };
        return (filename.escaped(),tagged.escaped());
    }
    let path=tagged.and_then(|p| p.local_path()).unwrap_or_else(|| row.source_path.clone());
    let name=path.file_name().unwrap_or(path.as_os_str()).to_string_lossy().into_owned();
    (name,path.display().to_string())
}
fn timestamp(row: &Candidate) -> String {
    match &row.timestamp {
        CandidateTime::Checkpoint(ms) => format!("checkpoint {}",utc_time(*ms / 1000)),
        CandidateTime::LegacyMtime(Some(t)) => t.duration_since(std::time::UNIX_EPOCH)
            .map(|d| format!("legacy file mtime {}", utc_time(d.as_secs())))
            .unwrap_or_else(|_| "legacy time unknown".into()),
        CandidateTime::LegacyMtime(None) => "legacy time unknown".into(),
        CandidateTime::Unknown => "time unknown".into(),
    }
}
fn scroll_path(editor: &mut Editor, right: bool) {
    let path_width=editor.active().view.area.0.min(88).saturating_sub(21) as usize;
    let Some(p)=editor.recovery_picker.as_mut() else { return; };
    let Some(row)=p.rows.get(p.cursor) else { return; };
    let length=display_paths(row).1.chars().count();
    p.path_scroll=if right { p.path_scroll.saturating_add(16).min(length.saturating_sub(path_width.max(1))) }
        else { p.path_scroll.saturating_sub(16) };
}
fn paint_row(frame: &mut ratatui::Frame, area: Rect, row: &Candidate,
    report: Option<&str>, selected: bool, style: ratatui::style::Style) {
    use ratatui::layout::{Constraint,Layout};
    use ratatui::widgets::Paragraph;
    // Independent clipped fields prevent arbitrarily long filenames from consuming metadata.
    let fields = Layout::horizontal([Constraint::Length(4), Constraint::Percentage(27),
        Constraint::Percentage(43), Constraint::Fill(1)]).split(area);
    let (name,_)=display_paths(row);
    let name=if row.busy && row.association.is_none() && row.provenance.is_none() {
        "Recovery files in use".into()
    } else { name };
    let availability=if row.busy { "busy: in use" }
        else { report.or(row.unavailable.as_deref()).unwrap_or("available") };
    for (text,rect) in [
        (format!("[{}]",if selected { 'x' } else { ' ' }),fields[0]),
        (name,fields[1]),(timestamp(row),fields[2]),(availability.to_owned(),fields[3]),
    ] { frame.render_widget(Paragraph::new(text).style(style),rect); }
}
fn paint_details(frame: &mut ratatui::Frame, area: Rect, row: &Candidate,
    report: Option<&str>, offset: usize, style: ratatui::style::Style) {
    use ratatui::layout::{Constraint,Layout};
    use ratatui::widgets::{Paragraph,Wrap};
    let fields = Layout::vertical([Constraint::Length(1), Constraint::Length(1),
        Constraint::Length(2), Constraint::Fill(1)]).split(area);
    let path=display_paths(row).1.chars().skip(offset).collect::<String>();
    frame.render_widget(Paragraph::new(format!("Path (Left/Right): {path}")).style(style),fields[0]);
    frame.render_widget(Paragraph::new(timestamp(row)).style(style),fields[1]);
    let availability=if row.busy {
        "An editor is using these recovery files. They cannot be opened while in use."
    } else { report.or(row.unavailable.as_deref()).unwrap_or("available") };
    frame.render_widget(Paragraph::new(availability).wrap(Wrap { trim: false }).style(style), fields[2]);
    frame.render_widget(Paragraph::new(row.preview.as_str()).wrap(Wrap {trim:false}).style(style),fields[3]);
}
/// Paint only bounded visible metadata and the highlighted row's capped preview.
pub(crate) fn paint(frame: &mut ratatui::Frame, editor: &mut Editor, styles: &crate::render::ChromeStyles) {
    use ratatui::widgets::{Block,Borders,Clear,Paragraph};
    let Some(p)=editor.recovery_picker.as_mut() else { return; };
    let (outer,rows,preview,footer)=geometry(frame.area()); window(p,rows.height);
    frame.render_widget(Clear,outer);
    frame.render_widget(Block::default().borders(Borders::ALL)
        .title("Review Recovery Files").style(styles.ov_query), outer);
    for (index,row) in p.rows.iter().enumerate().skip(p.top).take(rows.height as usize) {
        let style=if index==p.cursor {styles.overlay_selected} else {styles.ov_query};
        paint_row(frame, Rect::new(rows.x, rows.y + (index - p.top) as u16, rows.width, 1),
            row, p.reports[index].as_deref(), p.selected[index], style);
    }
    if let Some(row)=p.rows.get(p.cursor) {
        paint_details(frame,preview,row,p.reports[p.cursor].as_deref(),p.path_scroll,styles.ov_query);
    }
    frame.render_widget(Paragraph::new(p.message.as_str()).style(styles.ov_query),footer);
}
#[cfg(test)]
#[path = "recovery_picker/tests.rs"]
mod regression_tests;
