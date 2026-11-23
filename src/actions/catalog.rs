use std::sync::LazyLock;

use regex::{Captures, Regex};

use crate::desktop::ExternalAction;

use super::command::{
    BrowserAction, Button, Command, Direction, FileAction, InputAction, Landmark, MediaAction,
    RoutineAction, SystemAction, WindowControl,
};

type Rule = (&'static str, fn(&Captures<'_>) -> Option<Command>);

const RULES: &[Rule] = &[
    (
        r"^(?:start|set(?: mo)?|magtakda ng) (?:a |mo yung |yung )?timer (?:for |na )?(\d+) (second|segundo|minute|minuto|minutong|hour|oras)\w*(?: (?:named|called|may pangalang|na may pangalang) (.+))?$",
        |m| {
            Some(Command::Timer(
                duration(m, 1, 2)?,
                part(m, 3).unwrap_or("Timer").into(),
            ))
        },
    ),
    (
        r"^(?:remind (?:mo ako|me) in|paalalahanan ako pagkalipas ng) (\d+) (second|segundo|minute|minuto|hour|oras)\w* (?:to|na) (.+)$",
        |m| Some(Command::Reminder(duration(m, 1, 2)?, m[3].into())),
    ),
    (
        r"^(?:add (.+?) to my to-do list|add mo sa to-do list ko ang (.+)|idagdag ang (.+?) sa listahan ng gagawin ko)$",
        |m| Some(Command::Task(first(m)?.into())),
    ),
    (
        r#"^(?:save (?:a note saying|mo sa notes ang)|itala ang) ["“](.+?)["”]$"#,
        |m| Some(Command::Note(m[1].into())),
    ),
    (
        r"^(?:find (?:my note named|mo yung note named)|hanapin ang tala kong) (.+)$",
        |m| Some(Command::FindNote(m[1].into())),
    ),
    (
        r"^(?:find (?:our conversation|mo yung chat natin) about|hanapin ang usapan natin tungkol sa) (.+)$",
        |m| Some(Command::FindChat(m[1].into())),
    ),
    (
        r"^(?:open mo yung (.+?) (?:view )?sa amica|show me amica['’]s (.+?)(?: view)?|ipakita ang (.+?)(?: ng amica)?)$",
        |m| view(first(m)?),
    ),
    (
        r"^(?:start the screensaver after|simulan ang screensaver pagkatapos ng|set mo ang screensaver to start after) (\d+) (second|segundo|minute|minuto)\w* (?:of inactivity|na walang galaw|idle)$",
        |m| {
            Some(Command::SaverSetting(
                "saver_idle".into(),
                duration(m, 1, 2)?.to_string(),
            ))
        },
    ),
    (
        r"^(?:gamitin ang kulay na (.+?) para sa ambient|set ambient to the (.+?) palette|use the (.+?) color palette for ambient)$",
        |m| {
            let value = first(m)?.to_lowercase();
            let (head, tail) = value.split_at_checked(1)?;
            Some(Command::SaverSetting(
                "saver_palette".into(),
                format!("{}{tail}", head.to_uppercase()),
            ))
        },
    ),
    (
        r"^(?:for the dashboard screensaver[, ]+please |paki-)?(show|hide|ipakita ang|itago ang) (?:the )?(clock|date|orb|next timer|media status|status ng media|petsa)(?: (?:on the|sa) dashboard screensaver)?$",
        |m| {
            Some(Command::SaverSetting(
                match m[2].to_lowercase().as_str() {
                    "next timer" => "show_timer",
                    "media status" | "status ng media" => "show_media",
                    "petsa" => "show_date",
                    "orb" => "show_orb",
                    "date" => "show_date",
                    _ => "show_clock",
                }
                .into(),
                if matches!(m[1].to_lowercase().as_str(), "hide" | "itago ang") {
                    "off"
                } else {
                    "on"
                }
                .into(),
            ))
        },
    ),
    (
        r"^(?:start|open|i-on ang|simulan ang) (?:the )?(?:(ambient|dashboard|black clock) )?screensaver$",
        |m| {
            Some(Command::Saver(
                match part(m, 1).map(str::to_lowercase).as_deref() {
                    Some("ambient") => "Ambient",
                    Some("dashboard") => "Dashboard",
                    Some("black clock") => "Black Clock",
                    _ => "",
                }
                .into(),
            ))
        },
    ),
    (
        r"^(?:move (?:mo )?(?:the |ang )?(?:mouse|pointer|cursor)|iusog (?:mo )?ang (?:mouse|pointer|cursor))(?: (?:by|nang))? (\d+) pixels? (\w+)$",
        |m| {
            input(InputAction::Move(
                Direction::parse(&m[2])?,
                m[1].parse().ok()?,
            ))
        },
    ),
    (
        r"^(?:drag (?:mo )?from here|i-drag mula rito nang) (\d+) pixels? (\w+)$",
        |m| {
            input(InputAction::Drag(
                Direction::parse(&m[2])?,
                m[1].parse().ok()?,
            ))
        },
    ),
    (
        r"^(?:move the cursor to|il(i|a)pat ang cursor sa|punta ang cursor sa) (?:the )?(.+?) (?:in this window|of this window|ng window na ito|ng current window|inside the window)$",
        |m| input(InputAction::Point(landmark(&m[2])?)),
    ),
    (
        r"^(?:(?:horizontal )?scroll(?: mo)?(?: horizontally)?|i-scroll(?: nang pahalang)?) (\w+)(?: nang)?(?: ([\w-]+) (?:notch(?:es)?|ikot))?(?: (?:in|sa|on) (.+))?$",
        |m| {
            Some(Command::Input(
                part(m, 3).unwrap_or("").into(),
                InputAction::Scroll(
                    Direction::parse(&m[1])?,
                    part(m, 2).and_then(count).unwrap_or(1) as i32,
                ),
            ))
        },
    ),
    (
        r"^(?:click(?: mo yung)? grid cell|i-click ang kahon) (\d+)(?: sa grid)?$",
        |m| input(InputAction::Cell(m[1].parse().ok()?)),
    ),
    (
        r"^(?:show(?: mo yung| a)?|ipakita ang) (\d+) by (\d+) (?:na grid para sa mouse|mouse grid)$",
        |m| input(InputAction::Grid(m[1].parse().ok()?, m[2].parse().ok()?)),
    ),
    (
        r"^(?:(?:i-)?(left-click|right-click|middle-click|double-click|click))(?: mo)? (?:the |ang |yung )?(.+)$",
        |m| {
            let button = match m[1].to_lowercase().as_str() {
                "right-click" => Button::Right,
                "middle-click" => Button::Middle,
                "double-click" => Button::Double,
                _ => Button::Left,
            };
            let label = (!matches!(
                m[2].to_lowercase().as_str(),
                "here" | "dito" | "rito" | "current pointer"
            ))
            .then(|| m[2].to_owned());
            input(InputAction::Click(button, label))
        },
    ),
    (
        r"^(?:hover over|hover ka sa|itapat ang cursor sa) (?:the |ang )?(.+)$",
        |m| input(InputAction::Hover(m[1].into())),
    ),
    (
        r"^(?:press (?:the key combination |mo ang shortcut na |mo once ang )?|pindutin nang (?:isang beses|sabay) ang )(.+?)(?: once)?$",
        |m| input(InputAction::Keys(m[1].into(), 1)),
    ),
    (
        r"^(?:repeat (.+?) (\w+) (?:times )?in the current window|(?:ulitin sa kasalukuyang window|repeat mo sa current window) ang (.+?) (\w+)(?: times)?)$",
        |m| {
            input(InputAction::Keys(
                part(m, 1).or_else(|| part(m, 3))?.into(),
                count(part(m, 2).or_else(|| part(m, 4))?)?.try_into().ok()?,
            ))
        },
    ),
    (
        r#"^(?:type(?: mo ang)?|i-type ang) ["“](.+?)["”](?: (?:into|in|sa) (?:the )?.+)?$"#,
        |m| input(InputAction::Text(m[1].into())),
    ),
    (
        r"^(?:insert(?: mo ang| an| a)?|ilagay ang) (peso sign|euro sign|em dash|en dash|degree sign|copyright (?:sign|symbol)|trademark symbol|at sign|question mark|semicolon|check mark|bullet|ellipsis(?: character)?)(?: (?:in|into|sa) .+)?$",
        |m| {
            input(InputAction::Text(
                match m[1].to_lowercase().as_str() {
                    "peso sign" => "₱",
                    "euro sign" => "€",
                    "em dash" => "—",
                    "en dash" => "–",
                    "degree sign" => "°",
                    "copyright symbol" | "copyright sign" => "©",
                    "at sign" => "@",
                    "question mark" => "?",
                    "semicolon" => ";",
                    "trademark symbol" => "™",
                    "check mark" => "✓",
                    "bullet" => "•",
                    _ => "…",
                }
                .into(),
            ))
        },
    ),
    (
        r"^(?:extend (?:the text selection|mo ang selection)) to the (start|end) of the line$",
        |m| input(InputAction::SelectLine(m[1].eq_ignore_ascii_case("end"))),
    ),
    (
        r"^(?:extend (?:the text selection|mo ang selection)|palawakin ang piniling teksto nang) (\w+) (characters?|karakter|words?|salita|lines?|linya) (\w+)$",
        |m| {
            input(InputAction::Select(
                Direction::parse(&m[3])?,
                count(&m[1])?.try_into().ok()?,
                matches!(m[2].to_lowercase().as_str(), "word" | "words" | "salita"),
            ))
        },
    ),
    (
        r"^(?:focus (?:the )?|i-focus mo ang |ilipat ang focus sa )(.+?)(?: without clicking(?: it)?)?$",
        |m| input(InputAction::Focus(m[1].into())),
    ),
    (
        r"^(?:set the selected slider to (\d+)|itakda ang napiling slider sa (\d+)|set mo sa (\d+))(?: percent|%)(?: ang napiling slider)?$",
        |m| input(InputAction::Slider(first(m)?.parse().ok()?)),
    ),
    (
        r"^(?:select (?:mo ang )?item|piliin ang bilang) (\d+)(?: in this list| sa list na ito| sa listahang ito)$",
        |m| input(InputAction::SelectItem(m[1].parse().ok()?)),
    ),
    (
        r"^(?:activate(?: mo)? (?:the |ang |yung )?|i-activate ang )(.+?) (?:sa active window|in this window|sa window na ito)$",
        |m| {
            if let Some((number, role)) = m[1].split_once(' ')
                && let Some(number) = count(number)
            {
                return input(InputAction::Role(role.into(), number));
            }
            input(InputAction::Invoke(m[1].into()))
        },
    ),
    (
        r"^(?:show(?: me| mo ang)? (?:the )?|ipakita ang )(.+?)(?: (?:ng|of the) focused control| ng kasalukuyang control)$",
        |m| input(InputAction::Inspect(m[1].into())),
    ),
    (r"^show the focused control['’]s (.+)$", |m| {
        input(InputAction::Inspect(m[1].into()))
    }),
    (
        r"^(?:toggle (?:the option labeled|mo yung option na)|i-toggle ang opsyong) (.+)$",
        |m| input(InputAction::Toggle(m[1].into())),
    ),
    (
        r"^(?:(expand|collapse) |i-(expand|collapse) mo yung |(palawakin|tiklopin) ang )(.+)$",
        |m| {
            input(InputAction::Expand(
                m[4].into(),
                !matches!(first(m)?.to_lowercase().as_str(), "collapse" | "tiklopin"),
            ))
        },
    ),
    (
        r"^(?:show me the first|show mo yung first|ipakita ang unang) (\d+) (?:accessible )?items? (?:in this list|dito|sa listahang ito)$",
        |m| input(InputAction::List(m[1].parse().ok()?)),
    ),
    (
        r"^(?:highlight (?:my pointer with a|mo ang cursor with)|markahan ang cursor gamit ang) (.+?)(?: ring)?$",
        |m| input(InputAction::Mark(m[1].into())),
    ),
    (
        r"^(?:take a local screenshot of (.+)|screenshot mo ang (.+?) locally|kumuha ng screenshot ng (.+?) at itago rito sa pc)$",
        |m| Some(Command::Capture(first(m)?.into())),
    ),
    (
        r#"^(?:find(?: mo ang)?|hanapin ang) ["“](.+?)["”] (?:on this page|sa page na ito|sa pahinang ito|in this document|sa dokumentong ito|sa document)$"#,
        |m| input(InputAction::Find(m[1].into())),
    ),
    (
        r"^(?:paste(?: mo yung)? (?:my )?clipboard|i-paste ang nasa clipboard) (?:into (?:the )?|sa (?:the )?)(.+)$",
        |m| Some(Command::Input(m[1].into(), InputAction::Paste)),
    ),
    (
        r#"^(?:replace (?:the selected text with|mo yung selection with)|palitan ang napiling teksto ng) ["“](.+?)["”]$"#,
        |m| input(InputAction::Replace(m[1].into())),
    ),
    (
        r"^(?:convert (?:the selected text|mo yung selection) to (.+)|gawing (.+?) ang napiling teksto)$",
        |m| input(InputAction::Case(first(m)?.into())),
    ),
    (
        r"^(?:insert (?:mo dito ang )?|magdagdag dito ng )(\w+)(?: na)? line breaks?(?: here)?$",
        |m| {
            input(InputAction::Text(
                "\n".repeat(usize::from(count(&m[1])?.min(12))),
            ))
        },
    ),
    (r"^(?:switch (?:to|mo sa)|lumipat sa) (.+)$", |m| {
        let name = m[1].to_lowercase();
        if name.starts_with("tab named ")
            || name.starts_with("the browser tab ")
            || name.starts_with("browser tab ")
            || name.contains(" at ") && name.ends_with(" volume")
        {
            return None;
        }
        window(&m[1], WindowControl::Switch)
    }),
    (
        r"^(?:find (?:the window named|mo ang window named)|hanapin ang window na pinamagatang) (.+)$",
        |m| window(&m[1], WindowControl::Find),
    ),
    (
        r"^(?:i-)?(minimize|maximize|restore|close|palakihin|isara) (?:mo )?(?:the |ang )?(?:normal size ng |window ng )?(.+?)(?: window)?(?: to normal size)?$",
        |m| {
            (!m[2].to_lowercase().contains(" layout"))
                .then(|| ())
                .and_then(|()| {
                    window(
                        &m[2],
                        WindowControl::Mode(match m[1].to_lowercase().as_str() {
                            "minimize" => ExternalAction::Minimize,
                            "maximize" | "palakihin" => ExternalAction::Maximize,
                            "restore" => ExternalAction::Restore,
                            _ => ExternalAction::Close,
                        }),
                    )
                })
        },
    ),
    (r"^ibalik sa karaniwang laki ang window ng (.+)$", |m| {
        window(&m[1], WindowControl::Mode(ExternalAction::Restore))
    }),
    (
        r"^(?:snap (?:this window to|mo itong window sa)|i-snap ang window na ito sa) (?:the )?(.+)$",
        |m| window("", WindowControl::Snap(landmark(&m[1])?)),
    ),
    (
        r"^(?:move (?:mo ang )?|ilipat ang )(.+?) (?:to|sa) (?:monitor (\d+)|second monitor)$",
        |m| {
            window(
                &m[1],
                WindowControl::Display(part(m, 2).unwrap_or("2").parse().ok()?),
            )
        },
    ),
    (
        r"^(?:resize (?:this window to|mo itong window to)|baguhin ang laki ng window na ito sa) (\d+) (?:by|sa|x) (\d+)$",
        |m| {
            window(
                "",
                WindowControl::Resize(m[1].parse().ok()?, m[2].parse().ok()?),
            )
        },
    ),
    (
        r"^(?:take me to|dalhin mo ako sa|open mo yung folder na) (.+)$",
        |m| Some(Command::Files(FileAction::OpenFolder(m[1].into()))),
    ),
    (
        r"^(?:make a folder called (.+?) here|create ka ng folder named (.+?) dito|gumawa rito ng folder na (.+))$",
        |m| Some(Command::Files(FileAction::Create(first(m)?.into()))),
    ),
    (
        r"^(?:find files with (.+?) in the name here|search mo dito ang files na may (.+?) sa filename|hanapin dito ang mga file na may (.+?) sa pangalan)$",
        |m| Some(Command::Files(FileAction::Find(first(m)?.into()))),
    ),
    (
        r"^(?:show only (.+?) files here|ipakita lang dito ang mga (.+?) file|filter ang folder na ito to (.+?) files)$",
        |m| Some(Command::Files(FileAction::Filter(first(m)?.into()))),
    ),
    (
        r"^(?:sort (?:this folder|mo ang folder) by|ayusin ang listahan ng file ayon sa) (.+)$",
        |m| Some(Command::Files(FileAction::Sort(m[1].into()))),
    ),
    (
        r"^(?:set this folder['’]s view to (.+)|change mo ang folder view to (.+)|ipakita ang folder na ito gamit ang (.+?) view)$",
        |m| Some(Command::Files(FileAction::View(first(m)?.into()))),
    ),
    (
        r"^(?:copy (?:the selected file to|mo yung selected file sa)|kopyahin ang napiling file sa) (.+)$",
        |m| Some(Command::Files(FileAction::Copy(m[1].into()))),
    ),
    (
        r"^(?:move (?:the selected file to|mo yung selected file sa)|ilipat ang napiling file sa) (.+)$",
        |m| Some(Command::Files(FileAction::Move(m[1].into()))),
    ),
    (
        r"^(?:rename the selected file to (.+?) and keep its extension|rename mo yung file as (.+?) with the same extension|palitan ang pangalan ng file ng (.+?) at panatilihin ang extension)$",
        |m| Some(Command::Files(FileAction::Rename(first(m)?.into()))),
    ),
    (
        r"^(?:move (.+?) to the recycle bin|ilipat ang (.+?) sa recycle bin|send mo ang (.+?) sa recycle bin)$",
        |m| Some(Command::Files(FileAction::Recycle(first(m)?.into()))),
    ),
    (
        r"^(?:open mo sa notepad ang (.+)|(?:open|buksan ang) (.+?) (?:in|sa) notepad)$",
        |m| Some(Command::Files(FileAction::Edit(first(m)?.into()))),
    ),
    (
        r"^(?:start a new text document named|gumawa ng bagong text document na|new text document na) (.+)$",
        |m| Some(Command::Files(FileAction::NewText(m[1].into()))),
    ),
    (
        r"^(?:save a copy of this text document as|mag-save ng kopya ng dokumentong ito bilang|save a copy nito as) (.+)$",
        |m| Some(Command::Files(FileAction::SaveCopy(m[1].into()))),
    ),
    (
        r"^(?:play (?:the local file|mo yung local file na)|patugtugin ang file na) (.+)$",
        |m| Some(Command::Files(FileAction::Play(m[1].into()))),
    ),
    (
        r"^(?:buksan sa browser ang|open(?: mo ang)?|go to|buksan ang) (https://\S+?)(?: (?:in my browser|sa browser|in a new tab|sa bagong tab|sa new tab))?$",
        |m| Some(Command::Browser(BrowserAction::Open(m[1].into()))),
    ),
    (
        r"^(?:search the web for|hanapin sa web ang|i-search mo sa browser ang) (.+)$",
        |m| Some(Command::Browser(BrowserAction::Search(m[1].into()))),
    ),
    (
        r"^(?:switch to the browser tab titled|switch mo sa tab named|lumipat sa browser tab na) (.+)$",
        |m| Some(Command::Browser(BrowserAction::Tab(m[1].into()))),
    ),
    (
        r"^(?:set browser zoom to|itakda ang browser zoom sa|set mo yung browser zoom to) (\d+)%$",
        |m| Some(Command::Browser(BrowserAction::Zoom(m[1].parse().ok()?))),
    ),
    (
        r"^(?:go (back|forward) by (\w+) pages? sa tab history|go (\w+) pages? (back|forward) in this tab['’]s history|(bumalik|sumulong) nang (\w+)(?: na)? pahina sa history ng tab na ito)$",
        |m| {
            let direction = part(m, 1)
                .or_else(|| part(m, 4))
                .or_else(|| part(m, 5))?
                .to_lowercase();
            let count =
                count(part(m, 2).or_else(|| part(m, 3)).or_else(|| part(m, 6))?)?.min(10) as i8;
            Some(Command::Browser(BrowserAction::History(
                if matches!(direction.as_str(), "back" | "bumalik") {
                    -count
                } else {
                    count
                },
            )))
        },
    ),
    (
        r"^(?:bookmark this page as|i-bookmark ang pahinang ito bilang|bookmark mo itong page as) (.+)$",
        |m| Some(Command::Browser(BrowserAction::Bookmark(m[1].into()))),
    ),
    (
        r"^(?:show me and reopen up to|ipakita at muling buksan ang hanggang|preview and reopen up to) (\w+)(?: na)? (?:pages?|pahina) (?:from .+?browser history|mula sa history)$",
        |m| {
            Some(Command::Browser(BrowserAction::Reopen(
                count(&m[1])?.try_into().ok()?,
            )))
        },
    ),
    (
        r#"^(?:copy ["“](.+?)["”] to my clipboard|copy mo sa clipboard ang ["“](.+?)["”]|kopyahin ang ["“](.+?)["”] sa clipboard ko)$"#,
        |m| Some(Command::Clipboard(first(m)?.into())),
    ),
    (
        r"^(?:copy the title of the (.+?) window|copy mo ang (.+?) window title|kopyahin ang pamagat ng window ng (.+))$",
        |m| Some(Command::CopyTitle(first(m)?.into())),
    ),
    (
        r"^(?:set (?:windows volume|mo ang volume) to|itakda ang lakas ng tunog sa) (\d+)%$",
        |m| {
            Some(Command::System(SystemAction::Volume(
                m[1].parse().ok()?,
                false,
            )))
        },
    ),
    (
        r"^(?:adjust (?:mo ang )?windows volume (up|down) (\d+) percentage points|baguhin ang lakas ng tunog nang (\d+) puntos (pataas|pababa))$",
        |m| {
            let direction = Direction::parse(part(m, 1).or_else(|| part(m, 4))?)?;
            let percent: i32 = part(m, 2).or_else(|| part(m, 3))?.parse().ok()?;
            Some(Command::System(SystemAction::Volume(
                if direction == Direction::Down {
                    -percent
                } else {
                    percent
                },
                true,
            )))
        },
    ),
    (
        r"^(?:set (?:mo ang )?(.+?) volume to (\d+)%|itakda sa (\d+)% ang lakas ng tunog ng (.+))$",
        |m| {
            Some(Command::System(SystemAction::AppVolume(
                part(m, 1).or_else(|| part(m, 4))?.into(),
                part(m, 2).or_else(|| part(m, 3))?.parse().ok()?,
            )))
        },
    ),
    (
        r"^(?:mute (.+?) in the windows mixer|patahimikin ang (.+?) sa windows mixer|i-mute mo yung (.+?) lang sa mixer)$",
        |m| Some(Command::System(SystemAction::Mute(first(m)?.into()))),
    ),
    (
        r"^(?:ilipat ang tunog ng windows sa|switch (?:mo ang windows audio output|windows sound output) to) (.+)$",
        |m| Some(Command::System(SystemAction::Output(m[1].into(), None))),
    ),
    (
        r"^(?:switch to (.+?) at (\d+)% volume|use (.+?) and set it to (\d+)% volume|gamitin ang (.+?) at itakda sa (\d+)% ang tunog)$",
        |m| {
            Some(Command::System(SystemAction::Output(
                part(m, 1)
                    .or_else(|| part(m, 3))
                    .or_else(|| part(m, 5))?
                    .into(),
                Some(
                    part(m, 2)
                        .or_else(|| part(m, 4))
                        .or_else(|| part(m, 6))?
                        .parse()
                        .ok()?,
                ),
            )))
        },
    ),
    (
        r"^(?:ipakita ang (gamit ng cpu|gamit ng memory|bilis ng download|bilis ng upload|kasalukuyang pinagmumulan ng kuryente|default na audio output) ng pc na ito|show mo yung (cpu usage|memory usage|download rate|upload rate|battery percentage|display resolution) ng pc|show me (cpu usage|ram usage|download rate|upload rate|free disk space|number of connected monitors) on this pc)$",
        |m| {
            Some(Command::System(SystemAction::Metric(
                first(m)?.to_lowercase(),
            )))
        },
    ),
    (
        r"^(?:set (?:this display['’]s brightness|mo yung screen brightness) to (\d+)%|itakda sa (\d+)% ang liwanag ng screen na ito)$",
        |m| {
            Some(Command::System(SystemAction::Brightness(
                first(m)?.parse().ok()?,
            )))
        },
    ),
    (
        r"^(?:show me windows settings for|ipakita ang mga setting ng windows para sa|open mo yung windows settings for|show me where to change|ipakita kung saan mababago ang|show me yung setting for) (.+)$",
        |m| Some(Command::System(SystemAction::Settings(m[1].into()))),
    ),
    (
        r"^(?:set (?:windows magnifier|mo ang magnifier) to|itakda ang windows magnifier sa) (\d+)%$",
        |m| Some(Command::System(SystemAction::Magnifier(m[1].parse().ok()?))),
    ),
    (
        r"^(?:pause (?:this player (?:after|in))|i-pause ang player pagkalipas ng) (\d+) (minute|minuto)\w*$",
        |m| Some(Command::Media(MediaAction::PauseLater(duration(m, 1, 2)?))),
    ),
    (
        r"^(?:pause (?:playback in|mo sa)|ihinto muna ang pagtugtog sa) (.+)$",
        |m| Some(Command::Media(MediaAction::Play(m[1].into(), false))),
    ),
    (
        r"^(?:resume (?:playback in|mo yung playback sa)|ipagpatuloy ang pagtugtog sa) (.+)$",
        |m| Some(Command::Media(MediaAction::Play(m[1].into(), true))),
    ),
    (
        r"^(?:seek (?:mo )?(\d+) seconds? (forward|backward|back) (?:sa player|in the active player)|ilipat ang playback nang (\d+) segundo (pasulong|paatras|pabalik))$",
        |m| {
            let number: i32 = part(m, 1).or_else(|| part(m, 3))?.parse().ok()?;
            let backward = matches!(
                part(m, 2).or_else(|| part(m, 4))?.to_lowercase().as_str(),
                "backward" | "back" | "paatras" | "pabalik"
            );
            Some(Command::Media(MediaAction::Seek(
                if backward { -number } else { number },
                true,
            )))
        },
    ),
    (
        r"^(?:jump (?:ka )?to|tumalon sa oras na) (\d+):(\d+)(?: in this media file| sa file na ito| sa current media)$",
        |m| {
            Some(Command::Media(MediaAction::Seek(
                m[1].parse::<i32>()
                    .ok()?
                    .checked_mul(60)?
                    .checked_add(m[2].parse().ok()?)?,
                false,
            )))
        },
    ),
    (
        r"^(?:set (?:the active player['’]s speed|mo yung playback speed) to|itakda ang bilis ng playback sa) ([\d.]+)x$",
        |m| {
            Some(Command::Media(MediaAction::Rate(
                (m[1].parse::<f32>().ok()? * 100.0) as u16,
            )))
        },
    ),
    (
        r"^(?:i-search mo sa (youtube|spotify) ang (.+)|(?:find|hanapin ang) (.+?) (?:on|sa) (youtube|spotify))$",
        |m| {
            Some(Command::Media(MediaAction::Search(
                part(m, 1).or_else(|| part(m, 4))?.into(),
                part(m, 2).or_else(|| part(m, 3))?.into(),
            )))
        },
    ),
    (
        r"^(?:show the current player['’]s (.+)|show mo ang (.+?) ng current player|ipakita ang (.+?) ng kasalukuyang player)$",
        |m| Some(Command::Media(MediaAction::Status(first(m)?.into()))),
    ),
    (
        r"^(?:run my (.+?) routine|run mo ang (.+?) routine ko|patakbuhin ang routine kong (.+))$",
        |m| Some(Command::Routines(RoutineAction::Run(first(m)?.into()))),
    ),
    (
        r"^(?:preview the steps for my (.+?) routine|preview mo yung steps ng (.+?) routine ko|ipakita ang mga hakbang ng routine kong (.+))$",
        |m| Some(Command::Routines(RoutineAction::Preview(first(m)?.into()))),
    ),
    (
        r"^(?:set (?:a keyboard|mo a) shortcut for my (.+?) routine|magtakda ng shortcut para sa routine kong (.+))$",
        |m| Some(Command::Routines(RoutineAction::Shortcut(first(m)?.into()))),
    ),
    (
        r"^(?:save this (?:window )?layout as|i-save ang ayos ng mga window bilang) (.+)$",
        |m| Some(Command::Routines(RoutineAction::SaveLayout(m[1].into()))),
    ),
    (
        r"^(?:restore my (.+?) layout|restore mo yung (.+?) layout ko|ibalik ang layout kong (.+))$",
        |m| {
            Some(Command::Routines(RoutineAction::RestoreLayout(
                first(m)?.into(),
            )))
        },
    ),
    (
        r"^(?:add a step to open (.+?) in my (.+?) routine|add an open-(.+?) step sa (.+?) routine ko|idagdag sa routine kong (.+?) ang pagbukas ng (.+))$",
        |m| {
            Some(Command::Routines(RoutineAction::AddApp(
                part(m, 2)
                    .or_else(|| part(m, 4))
                    .or_else(|| part(m, 5))?
                    .into(),
                part(m, 1)
                    .or_else(|| part(m, 3))
                    .or_else(|| part(m, 6))?
                    .into(),
            )))
        },
    ),
    (
        r"^(?:add a (\d+)-second wait(?: step)? (?:to my|sa) (.+?) routine(?: ko)?|magdagdag ng paghintay na (\d+) segundo sa routine kong (.+))$",
        |m| {
            Some(Command::Routines(RoutineAction::Wait(
                part(m, 2).or_else(|| part(m, 4))?.into(),
                part(m, 1).or_else(|| part(m, 3))?.parse().ok()?,
            )))
        },
    ),
    (
        r"^(?:remove (?:mo yung )?step|alisin ang hakbang) (\d+) (?:from my (.+?) routine|sa (.+?) routine ko|sa routine kong (.+))$",
        |m| {
            Some(Command::Routines(RoutineAction::Remove(
                part(m, 2)
                    .or_else(|| part(m, 3))
                    .or_else(|| part(m, 4))?
                    .into(),
                m[1].parse().ok()?,
            )))
        },
    ),
    (
        r"^(?:duplicate (?:the|mo yung) routine named|gumawa ng kopya ng routine na) (.+)$",
        |m| Some(Command::Routines(RoutineAction::Duplicate(m[1].into()))),
    ),
    (
        r"^(?:show the last run of my (.+?) routine|show mo yung last run ng (.+?) routine ko|ipakita ang huling pagpapatakbo ng routine kong (.+))$",
        |m| Some(Command::Routines(RoutineAction::LastRun(first(m)?.into()))),
    ),
    (r"^(?:open (?:mo ang |the )?|buksan ang )(.+)$", |m| {
        view(&m[1]).or_else(|| app(&m[1]).map(|name| Command::Launch(name.into())))
    }),
    (
        r"^(?:can i (?:vent|talk) about|puwede ba nating pag-usapan|help (?:me (?:plan|decide|choose|reflect)|mo akong mag-plan)|tulungan mo akong|brainstorm|mag-isip tayo|explain|ipaliwanag|practice|magsanay|draft|gumawa ng maikling draft|quiz me|i-quiz mo ako|tanungin mo ako|give me a journal prompt|bigyan mo ako).+$",
        |_| Some(Command::Companion),
    ),
];

static PARSERS: LazyLock<Vec<(Regex, Rule)>> = LazyLock::new(|| {
    RULES
        .iter()
        .map(|&(pattern, parser)| {
            (
                Regex::new(&format!("(?i){pattern}")).unwrap(),
                (pattern, parser),
            )
        })
        .collect()
});

pub fn parse(input_text: &str) -> Option<Command> {
    let text = input_text.trim();
    if text.len() > 4_000 || text.contains('\n') {
        return None;
    }
    let lower = text.to_lowercase();
    if text.ends_with('?') {
        return [
            "can i vent about ",
            "can i talk about ",
            "puwede ba nating pag-usapan ",
        ]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
        .then_some(Command::Companion);
    }
    let mut text = text.trim_end_matches(['.', '!']).trim();
    for prefix in ["hey amica, ", "amica, ", "please ", "paki "] {
        if lower.starts_with(prefix)
            && let Some(rest) = text.get(prefix.len()..)
        {
            text = rest;
            break;
        }
    }
    let command = match text.to_lowercase().as_str() {
        "start focus" | "simulan ang focus" | "start mo ang focus" => {
            Command::Timer(1500, "Focus".into())
        }
        "stop now" | "stop desktop control" | "itigil agad" | "stop mo na" => Command::Stop,
        "start desktop control"
        | "simulan ang desktop control"
        | "start mo ang desktop control" => Command::DesktopMode(true),
        "open command palette" | "command palette" | "ipakita ang command palette" => {
            Command::Palette
        }
        "show grid" | "ipakita ang grid" | "show mo yung grid" => {
            Command::Input("".into(), InputAction::Grid(3, 3))
        }
        "click" | "click here" | "i-click dito" => {
            Command::Input("".into(), InputAction::Click(Button::Left, None))
        }
        "show tasks" | "show reminders" | "ipakita ang mga gawain" => Command::View("tasks".into()),
        "show recent memories" | "mga bagong alaala" | "ipakita ang mga bagong alaala" => {
            Command::View("recent memories".into())
        }
        "show routines" | "open routines" | "ipakita ang routines" => {
            Command::View("routines".into())
        }
        "start the screensaver" => Command::Saver("".into()),
        "shut down this pc" | "shut down the pc" => Command::Power("shutdown".into()),
        "restart this pc" | "restart the pc" => Command::Power("restart".into()),
        "sleep this pc" | "sleep the pc" => Command::Power("sleep".into()),
        _ => {
            return PARSERS.iter().find_map(|(pattern, (_, parser))| {
                pattern.captures(text).and_then(|m| parser(&m))
            });
        }
    };
    Some(command)
}

pub fn app(input: &str) -> Option<&'static str> {
    Some(match input.trim().to_lowercase().as_str() {
        "notepad" => "notepad",
        "calculator" => "calculator",
        "explorer" | "file explorer" => "file_explorer",
        "windows settings" | "settings" => "settings",
        "browser" | "default browser" => "browser",
        "youtube" => "youtube",
        "spotify" => "spotify",
        "paint" => "paint",
        "photos" => "photos",
        "clock" => "clock",
        "snipping tool" => "snipping_tool",
        "microsoft edge" | "edge" => "edge",
        "microsoft store" => "microsoft_store",
        _ => return None,
    })
}

fn view(name: &str) -> Option<Command> {
    let name = match name.trim().to_lowercase().as_str() {
        "home" => "home",
        "conversation" | "chat" => "conversation",
        "calendar" | "timers" | "time & date" => "time",
        "saved" => "saved",
        "memory map" => "memory map",
        "recent memories" | "mga bagong alaala" => "recent memories",
        "apps" => "apps",
        "settings" => "settings",
        "system specs" => "system specs",
        "command palette" => return Some(Command::Palette),
        "tasks" | "reminders" => "tasks",
        "routines" => "routines",
        _ => return None,
    };
    Some(Command::View(name.into()))
}

fn input(action: InputAction) -> Option<Command> {
    Some(Command::Input(String::new(), action))
}

fn window(name: &str, action: WindowControl) -> Option<Command> {
    Some(Command::Window(name.trim().into(), action))
}

fn landmark(name: &str) -> Option<Landmark> {
    Some(match name.trim().to_lowercase().as_str() {
        "center" | "middle" | "gitna" => Landmark::Center,
        "top-left" | "top-left corner" | "upper-left corner" | "itaas na kaliwang bahagi" => {
            Landmark::TopLeft
        }
        "top" | "top edge midpoint" | "upper half" | "top half" => Landmark::Top,
        "top-right"
        | "top-right corner"
        | "upper-right corner"
        | "upper-right quarter"
        | "itaas na kanang bahagi"
        | "itaas na kanang sulok" => Landmark::TopRight,
        "bottom-left"
        | "bottom-left corner"
        | "lower-left corner"
        | "lower-left quarter"
        | "ibaba na kaliwang bahagi"
        | "ibaba na kaliwang sulok" => Landmark::BottomLeft,
        "bottom" | "bottom edge midpoint" | "bottom half" | "lower half" => Landmark::Bottom,
        "bottom-right" | "bottom-right corner" | "lower-right corner" | "ibabang kanang bahagi" => {
            Landmark::BottomRight
        }
        "gitna ng title bar" | "title bar midpoint" => Landmark::TitleBar,
        "left third" => Landmark::LeftThird,
        "middle third" => Landmark::MiddleThird,
        "right third" | "ikatlong bahagi sa kanan" => Landmark::RightThird,
        "left two-thirds" => Landmark::LeftTwoThirds,
        "left" | "left half" | "left edge midpoint" | "kaliwang kalahati" => Landmark::Left,
        "right" | "right half" | "right edge midpoint" | "kanang kalahati" => Landmark::Right,
        _ => return None,
    })
}

fn part<'a>(m: &Captures<'a>, index: usize) -> Option<&'a str> {
    m.get(index).map(|value| value.as_str().trim())
}

fn first<'a>(m: &Captures<'a>) -> Option<&'a str> {
    (1..m.len()).find_map(|index| part(m, index))
}

fn duration(m: &Captures<'_>, amount: usize, unit: usize) -> Option<u32> {
    let scale = match m[unit].to_lowercase().as_str() {
        "hour" | "oras" => 3600,
        "minute" | "minuto" | "minutong" => 60,
        _ => 1,
    };
    m[amount].parse::<u32>().ok()?.checked_mul(scale)
}

fn count(value: &str) -> Option<u16> {
    value.parse().ok().or_else(|| {
        Some(match value.to_lowercase().as_str() {
            "one" | "once" | "isang" | "first" => 1,
            "two" | "twice" | "dalawang" | "second" => 2,
            "three" | "tatlong" | "third" => 3,
            "four" | "apat" | "fourth" => 4,
            "five" | "limang" | "fifth" => 5,
            "six" | "anim" | "sixth" => 6,
            "seven" | "pitong" | "seventh" => 7,
            "eight" | "walong" | "eighth" => 8,
            "nine" | "siyam" | "ninth" => 9,
            "ten" | "sampung" | "tenth" => 10,
            _ => return None,
        })
    })
}

pub fn examples() -> Vec<crate::PaletteRow> {
    let mut outcome = "";
    include_str!("../../AMICA-PLAN.txt")
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if let Some(expected) = line.strip_prefix("Expected: ") {
                outcome = expected;
                return None;
            }
            let (number, example) = line.split_once(' ')?;
            if number.len() != 4 || number.parse::<u16>().is_err() {
                return None;
            }
            let (language, text) = example.split_once("] ")?;
            let command = parse(text)?;
            if matches!(command, Command::Companion) || !super::dispatcher::available(&command) {
                return None;
            }
            Some(crate::PaletteRow {
                phrase: text.into(),
                language: language.trim_start_matches('[').into(),
                outcome: outcome.into(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exercises_catalog_fixtures_and_reports_language_coverage() {
        let mut counts = [0; 3];
        let mut recognized = [0; 3];
        for line in include_str!("../../AMICA-PLAN.txt").lines().map(str::trim) {
            let Some((number, example)) = line.split_once(' ') else {
                continue;
            };
            if number.len() != 4 || number.parse::<u16>().is_err() {
                continue;
            }
            let (language, text) = example.split_once("] ").unwrap();
            let index = match language {
                "[EN" => 0,
                "[FIL" => 1,
                "[TG" => 2,
                _ => unreachable!(),
            };
            counts[index] += 1;
            if parse(text).is_some() {
                recognized[index] += 1;
            } else if std::env::var_os("AMICA_CATALOG_DETAILS").is_some() {
                eprintln!("{number} {language}] {text}");
            }
        }
        eprintln!(
            "Catalog recognized EN {}/{}, FIL {}/{}, TG {}/{}",
            recognized[0], counts[0], recognized[1], counts[1], recognized[2], counts[2]
        );
        assert_eq!(counts.iter().sum::<usize>(), 1_200);
        assert_eq!(recognized, counts);
        assert_eq!(
            parse("Scroll down two notches in Notepad"),
            Some(Command::Input(
                "Notepad".into(),
                InputAction::Scroll(Direction::Down, 2)
            ))
        );
        assert_eq!(
            parse("I-scroll pababa nang tatlong ikot."),
            Some(Command::Input(
                "".into(),
                InputAction::Scroll(Direction::Down, 3)
            ))
        );
        assert_eq!(
            parse("Set a timer for 5 minutes named Break."),
            Some(Command::Timer(300, "Break".into()))
        );
        assert_eq!(
            parse("Start the Black Clock screensaver"),
            Some(Command::Saver("Black Clock".into()))
        );
        assert_eq!(parse("Can you explain how to close Notepad?"), None);
    }
}
