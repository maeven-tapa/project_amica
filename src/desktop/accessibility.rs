use windows::Win32::{
    Foundation::{HWND, POINT},
    System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance},
    UI::Accessibility::*,
};

use crate::actions::command::{Direction, InputAction};

use super::{TargetWindow, input, windows::ComApartment};

const NOT_SUPPORTED: &str =
    "The selected app does not expose that operation through Windows accessibility.";

struct Surface {
    automation: IUIAutomation,
    root: IUIAutomationElement,
    target: TargetWindow,
    _apartment: ComApartment,
}

impl Surface {
    fn open(target: TargetWindow) -> Result<Self, &'static str> {
        input::focused(target)?;
        let apartment = ComApartment::initialize()?;
        unsafe {
            let automation: IUIAutomation =
                CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)
                    .or_else(|_| CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER))
                    .map_err(|_| NOT_SUPPORTED)?;
            let root = automation
                .ElementFromHandle(HWND(target.handle as _))
                .map_err(|_| NOT_SUPPORTED)?;
            Ok(Self {
                automation,
                root,
                target,
                _apartment: apartment,
            })
        }
    }

    fn matching(
        &self,
        role: Option<UIA_CONTROLTYPE_ID>,
    ) -> Result<Vec<IUIAutomationElement>, &'static str> {
        unsafe {
            let condition = if let Some(role) = role {
                self.automation.CreatePropertyCondition(
                    UIA_ControlTypePropertyId,
                    &windows::Win32::System::Variant::VARIANT::from(role.0),
                )
            } else {
                self.automation.CreateTrueCondition()
            }
            .map_err(|_| NOT_SUPPORTED)?;
            let elements = self
                .root
                .FindAll(TreeScope_Descendants, &condition)
                .map_err(|_| NOT_SUPPORTED)?;
            let len = elements.Length().map_err(|_| NOT_SUPPORTED)?;
            Ok((0..len.min(750))
                .filter_map(|index| elements.GetElement(index).ok())
                .filter(|element| visible(element))
                .collect())
        }
    }

    fn lookup(&self, query: &str) -> Result<IUIAutomationElement, String> {
        let (name, role) = named_role(query);
        let matches = self
            .matching(role)?
            .into_iter()
            .filter(|element| {
                unsafe { element.CurrentName().ok() }
                    .is_some_and(|label| label.to_string().trim().eq_ignore_ascii_case(name))
            })
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [element] => Ok(element.clone()),
            [] => Err(format!(
                "No accessible control named “{}”. Try the mouse grid.",
                query.trim()
            )),
            _ => Err(format!(
                "{} controls match “{}”. Use its exact name and role, or choose a mouse grid cell.",
                matches.len(),
                query.trim()
            )),
        }
    }

    fn focused(&self) -> Result<IUIAutomationElement, &'static str> {
        input::focused(self.target)?;
        unsafe {
            let element = self
                .automation
                .GetFocusedElement()
                .map_err(|_| NOT_SUPPORTED)?;
            let walker = self.automation.RawViewWalker().map_err(|_| NOT_SUPPORTED)?;
            let mut current = element.clone();
            for _ in 0..60 {
                if self
                    .automation
                    .CompareElements(&current, &self.root)
                    .ok()
                    .is_some_and(|same| same.as_bool())
                {
                    return Ok(element);
                }
                current = match walker.GetParentElement(&current) {
                    Ok(parent) => parent,
                    Err(_) => break,
                };
            }
        }
        Err("The focused control is outside the selected window.")
    }
}

fn visible(element: &IUIAutomationElement) -> bool {
    unsafe { element.CurrentIsOffscreen() }
        .ok()
        .is_some_and(|hidden| !hidden.as_bool())
}

pub fn scroll(
    target: TargetWindow,
    direction: Direction,
    notches: i32,
) -> Option<Result<bool, &'static str>> {
    let surface = Surface::open(target).ok()?;
    let vertical = matches!(direction, Direction::Up | Direction::Down);
    let amount = if matches!(direction, Direction::Up | Direction::Left) {
        ScrollAmount_SmallDecrement
    } else {
        ScrollAmount_SmallIncrement
    };
    let mut candidates = surface.focused().ok().into_iter().collect::<Vec<_>>();
    if let Ok(documents) = surface.matching(Some(UIA_DocumentControlTypeId)) {
        candidates.extend(documents);
    }
    candidates.push(surface.root.clone());
    if let Ok(panes) = surface.matching(Some(UIA_PaneControlTypeId)) {
        candidates.extend(panes);
    }
    for element in candidates {
        let Ok(pattern) = (unsafe {
            element.GetCurrentPatternAs::<IUIAutomationScrollPattern>(UIA_ScrollPatternId)
        }) else {
            continue;
        };
        let scrollable = unsafe {
            if vertical {
                pattern.CurrentVerticallyScrollable()
            } else {
                pattern.CurrentHorizontallyScrollable()
            }
        };
        if !scrollable.ok().is_some_and(|flag| flag.as_bool()) {
            continue;
        }
        let read_position = || {
            unsafe {
                if vertical {
                    pattern.CurrentVerticalScrollPercent()
                } else {
                    pattern.CurrentHorizontalScrollPercent()
                }
            }
            .ok()
        };
        let before = read_position();
        let mut accepted = false;
        for _ in 0..notches {
            if let Err(error) = input::focused(target) {
                return Some(Err(error));
            }
            let result = unsafe {
                if vertical {
                    pattern.Scroll(ScrollAmount_NoAmount, amount)
                } else {
                    pattern.Scroll(amount, ScrollAmount_NoAmount)
                }
            };
            if result.is_err() {
                break;
            }
            accepted = true;
        }
        if accepted {
            return Some(Ok(before
                .zip(read_position())
                .is_none_or(|(before, after)| before != after)));
        }
    }
    None
}

fn sensitive(element: &IUIAutomationElement) -> bool {
    unsafe { element.CurrentIsPassword() }.map_or(true, |password| password.as_bool())
}

fn named_role(query: &str) -> (&str, Option<UIA_CONTROLTYPE_ID>) {
    let lower = query.to_lowercase();
    for (suffix, role) in [
        (" button", UIA_ButtonControlTypeId),
        (" link", UIA_HyperlinkControlTypeId),
        (" checkbox", UIA_CheckBoxControlTypeId),
        (" check box", UIA_CheckBoxControlTypeId),
        (" edit", UIA_EditControlTypeId),
        (" box", UIA_EditControlTypeId),
        (" slider", UIA_SliderControlTypeId),
        (" tab", UIA_TabItemControlTypeId),
    ] {
        if lower.ends_with(suffix) {
            return (&query[..query.len() - suffix.len()], Some(role));
        }
    }
    for suffix in [" folder", " file", " photo"] {
        if lower.ends_with(suffix) {
            return (&query[..query.len() - suffix.len()], None);
        }
    }
    (query.trim(), None)
}

pub fn point(
    target: TargetWindow,
    name: &str,
) -> Result<windows_sys::Win32::Foundation::POINT, String> {
    let surface = Surface::open(target)?;
    let element = surface.lookup(name)?;
    let mut point = POINT::default();
    if unsafe { element.GetClickablePoint(&mut point) }
        .ok()
        .is_some_and(|success| success.as_bool())
    {
        Ok(windows_sys::Win32::Foundation::POINT {
            x: point.x,
            y: point.y,
        })
    } else {
        Err(NOT_SUPPORTED.into())
    }
}

fn editable_element(surface: &Surface) -> Result<IUIAutomationElement, &'static str> {
    let element = surface.focused()?;
    if sensitive(&element) {
        return Err("Choose a non-password text field.");
    }
    unsafe {
        if let Ok(value) =
            element.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
            && value
                .CurrentIsReadOnly()
                .ok()
                .is_some_and(|readonly| !readonly.as_bool())
        {
            return Ok(element);
        }
        if element
            .CurrentControlType()
            .ok()
            .is_some_and(|role| [UIA_EditControlTypeId, UIA_DocumentControlTypeId].contains(&role))
            && element
                .GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
                .is_ok()
        {
            return Ok(element);
        }
    }
    Err("Focus an editable field exposed by the selected app.")
}

pub fn editable(target: TargetWindow) -> Result<(), &'static str> {
    editable_element(&Surface::open(target)?).map(|_| ())
}

pub fn selection(target: TargetWindow) -> Result<(), &'static str> {
    selected_text(target).map(|_| ())
}

pub fn selected_text(target: TargetWindow) -> Result<String, &'static str> {
    let surface = Surface::open(target)?;
    let element = editable_element(&surface)?;
    let selected = unsafe {
        element
            .GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
            .and_then(|text| text.GetSelection())
            .and_then(|ranges| ranges.GetElement(0))
            .and_then(|range| range.GetText(1001))
    }
    .map_err(|_| NOT_SUPPORTED)?;
    let selected = selected.to_string();
    (!selected.is_empty() && selected.encode_utf16().count() <= 1000)
        .then_some(selected)
        .ok_or("Select one to 1,000 characters in the editable field first.")
}

pub fn find_field(target: TargetWindow) -> Result<(), &'static str> {
    let surface = Surface::open(target)?;
    let focused = editable_element(&surface)?;
    let name = unsafe { focused.CurrentName() }
        .map_err(|_| NOT_SUPPORTED)?
        .to_string()
        .to_lowercase();
    if ["find", "search", "hanapin"]
        .iter()
        .any(|word| name.contains(word))
    {
        Ok(())
    } else {
        Err("The app did not focus an accessible find field.")
    }
}

pub fn page_address(target: TargetWindow) -> Result<String, String> {
    let surface = Surface::open(target)?;
    for name in [
        "Address and search bar",
        "Search or enter address",
        "Search with Google or enter address",
    ] {
        if let Ok(address) = surface.lookup(name) {
            let value = unsafe {
                address
                    .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                    .and_then(|pattern| pattern.CurrentValue())
            }
            .map_err(|_| "The browser did not expose the page address.")?
            .to_string();
            let value = if value.contains("://") {
                value
            } else {
                format!("https://{value}")
            };
            let url = reqwest::Url::parse(&value)
                .map_err(|_| "The browser did not expose a valid page address.")?;
            if url.scheme() != "https"
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
            {
                return Err("Choose a regular HTTPS page before bookmarking.".into());
            }
            return Ok(url.to_string());
        }
    }
    Err("The browser did not expose the address bar.".into())
}

pub fn name_bookmark(target: TargetWindow, name: &str) -> Result<(), String> {
    let surface = Surface::open(target)?;
    let label = surface
        .lookup("Name box")
        .or_else(|_| surface.lookup("Name"))?;
    let done = surface
        .lookup("Done button")
        .or_else(|_| surface.lookup("Save button"))?;
    input::focused(target)?;
    unsafe {
        label
            .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
            .and_then(|pattern| pattern.SetValue(&windows::core::BSTR::from(name)))
    }
    .map_err(|_| "The browser did not expose a writable bookmark name.")?;
    invoke(&done)?;
    Ok(())
}

pub fn browser_zoom(target: TargetWindow) -> Option<u16> {
    let surface = Surface::open(target).ok()?;
    surface
        .matching(Some(UIA_ButtonControlTypeId))
        .ok()?
        .into_iter()
        .find_map(|button| {
            let label = unsafe { button.CurrentName().ok()? }
                .to_string()
                .to_lowercase();
            if !label.contains("zoom") {
                return None;
            }
            label
                .split(|ch: char| !ch.is_ascii_digit())
                .find_map(|word| word.parse::<u16>().ok())
        })
}

pub fn document_text(target: TargetWindow) -> Result<String, String> {
    let surface = Surface::open(target)?;
    let documents = surface.matching(Some(UIA_DocumentControlTypeId))?;
    let [document] = documents.as_slice() else {
        return Err("The editor did not expose exactly one visible document.".into());
    };
    if sensitive(document) {
        return Err("The document is protected.".into());
    }
    let text = unsafe {
        document
            .GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
            .and_then(|pattern| pattern.DocumentRange())
            .and_then(|range| range.GetText(1_048_577))
    }
    .map_err(|_| NOT_SUPPORTED)?;
    let text = text.to_string();
    if text.encode_utf16().count() > 1_048_576 {
        return Err(
            "Use the editor's Save As control for documents larger than one million characters."
                .into(),
        );
    }
    Ok(text)
}

fn entries(surface: &Surface) -> Result<Vec<IUIAutomationElement>, &'static str> {
    let walker = unsafe { surface.automation.ControlViewWalker() }.map_err(|_| NOT_SUPPORTED)?;
    let mut parent = surface.focused()?;
    for _ in 0..35 {
        let role = unsafe { parent.CurrentControlType() }.map_err(|_| NOT_SUPPORTED)?;
        if [
            UIA_ListControlTypeId,
            UIA_TreeControlTypeId,
            UIA_DataGridControlTypeId,
        ]
        .contains(&role)
        {
            return Surface {
                automation: surface.automation.clone(),
                root: parent,
                target: surface.target,
                _apartment: ComApartment::initialize()?,
            }
            .matching(Some(if role == UIA_ListControlTypeId {
                UIA_ListItemControlTypeId
            } else if role == UIA_TreeControlTypeId {
                UIA_TreeItemControlTypeId
            } else {
                UIA_DataItemControlTypeId
            }));
        }
        parent = unsafe { walker.GetParentElement(&parent) }
            .map_err(|_| "Focus an item in the intended list first.")?;
    }
    Err("Focus an item in the intended list first.")
}

pub fn apply(target: TargetWindow, action: &InputAction) -> Result<String, String> {
    let surface = Surface::open(target)?;
    let unavailable = |_| NOT_SUPPORTED;
    let result = match action {
        InputAction::Focus(name) => {
            let element = surface.lookup(name)?;
            input::focused(target)?;
            unsafe { element.SetFocus() }.map_err(unavailable)?;
            "Focused the control."
        }
        InputAction::Invoke(name) => {
            let element = surface.lookup(name)?;
            input::focused(target)?;
            invoke(&element)?;
            "Invoked the control."
        }
        InputAction::Toggle(name) => {
            let element = surface.lookup(name)?;
            input::focused(target)?;
            let state = unsafe {
                let toggle = element
                    .GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId)
                    .map_err(unavailable)?;
                toggle.Toggle().map_err(unavailable)?;
                toggle.CurrentToggleState().map_err(unavailable)?
            };
            return Ok(format!(
                "Toggled the option. State: {}.",
                if state == ToggleState_On {
                    "on"
                } else if state == ToggleState_Off {
                    "off"
                } else {
                    "mixed"
                }
            ));
        }
        InputAction::Expand(name, expand) => {
            let element = surface.lookup(name)?;
            input::focused(target)?;
            let pattern = unsafe {
                element.GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(
                    UIA_ExpandCollapsePatternId,
                )
            }
            .map_err(unavailable)?;
            if *expand {
                unsafe { pattern.Expand() }.map_err(unavailable)?;
                "Expanded the region."
            } else {
                unsafe { pattern.Collapse() }.map_err(unavailable)?;
                "Collapsed the region."
            }
        }
        InputAction::Slider(percent) => {
            if *percent > 100 {
                return Err("Use a number from 0 to 100 percent.".into());
            }
            let element = surface.focused()?;
            let pattern = unsafe {
                element
                    .GetCurrentPatternAs::<IUIAutomationRangeValuePattern>(UIA_RangeValuePatternId)
            }
            .map_err(unavailable)?;
            let (minimum, maximum) = unsafe {
                (
                    pattern.CurrentMinimum().map_err(unavailable)?,
                    pattern.CurrentMaximum().map_err(unavailable)?,
                )
            };
            if maximum <= minimum {
                return Err(NOT_SUPPORTED.into());
            }
            input::focused(target)?;
            unsafe {
                pattern.SetValue(minimum + (maximum - minimum) * f64::from(*percent) / 100.0)
            }
            .map_err(unavailable)?;
            let actual = unsafe { pattern.CurrentValue() }.map_err(unavailable)?;
            return Ok(format!(
                "Slider is at {:.0}%.",
                (actual - minimum) * 100.0 / (maximum - minimum)
            ));
        }
        InputAction::SelectItem(number) => {
            let index = usize::from(number.checked_sub(1).ok_or("Item numbers start at 1.")?);
            let entries = entries(&surface)?;
            let element = entries
                .get(index)
                .ok_or("That item is outside the visible list.")?;
            input::focused(target)?;
            unsafe {
                element
                    .GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(
                        UIA_SelectionItemPatternId,
                    )
                    .and_then(|pattern| pattern.Select())
                    .map_err(unavailable)?;
            }
            "Selected the item."
        }
        InputAction::Role(name, number) => {
            let role = match name.to_lowercase().trim_end_matches('s') {
                "button" => UIA_ButtonControlTypeId,
                "link" => UIA_HyperlinkControlTypeId,
                "check box" | "checkbox" => UIA_CheckBoxControlTypeId,
                "tab" => UIA_TabItemControlTypeId,
                "radio button" => UIA_RadioButtonControlTypeId,
                _ => return Err("Choose button, link, checkbox, tab, or radio button.".into()),
            };
            let index = usize::from(number.checked_sub(1).ok_or("Item numbers start at 1.")?);
            let elements = surface.matching(Some(role))?;
            let element = elements
                .get(index)
                .ok_or("That numbered control is unavailable.")?;
            input::focused(target)?;
            invoke(element)?;
            "Activated the numbered control."
        }
        InputAction::Inspect(property) => {
            let element = surface.focused()?;
            if sensitive(&element) {
                return Err("This control is protected.".into());
            }
            let value = unsafe {
                match property.to_lowercase().as_str() {
                    "name" => element.CurrentName().map_err(unavailable)?.to_string(),
                    "role" | "control type" => element.CurrentLocalizedControlType().map_err(unavailable)?.to_string(),
                    "enabled" | "enabled state" => element.CurrentIsEnabled().map_err(unavailable)?.as_bool().to_string(),
                    "focusable" | "keyboard focusable" => element.CurrentIsKeyboardFocusable().map_err(unavailable)?.as_bool().to_string(),
                    "automation id" => element.CurrentAutomationId().map_err(unavailable)?.to_string(),
                    _ => return Err("Available properties are name, role, enabled state, focusable, and automation ID.".into()),
                }
            };
            return Ok(format!("Focused control {property}: {value}"));
        }
        InputAction::List(count) => {
            let items = entries(&surface)?;
            let result = items
                .iter()
                .take(usize::from((*count).min(25)))
                .enumerate()
                .filter_map(|(index, item)| {
                    if sensitive(item) {
                        return None;
                    }
                    let name = unsafe { item.CurrentName().ok()? };
                    Some(format!("{}. {name}", index + 1))
                })
                .collect::<Vec<_>>();
            return if result.is_empty() {
                Err("The focused list exposes no readable names.".into())
            } else {
                Ok(result.join("\n"))
            };
        }
        _ => return Err(NOT_SUPPORTED.into()),
    };
    Ok(result.into())
}

fn invoke(element: &IUIAutomationElement) -> Result<(), &'static str> {
    unsafe {
        let name = element
            .CurrentName()
            .map_err(|_| NOT_SUPPORTED)?
            .to_string()
            .to_lowercase();
        if sensitive(element)
            || name.split(|ch: char| !ch.is_alphanumeric()).any(|word| {
                matches!(
                    word,
                    "delete"
                        | "recycle"
                        | "send"
                        | "submit"
                        | "buy"
                        | "purchase"
                        | "pay"
                        | "overwrite"
                        | "install"
                )
            })
        {
            return Err("Use the target app to review and confirm that operation.");
        }
        if let Ok(pattern) =
            element.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
        {
            pattern.Invoke().map_err(|_| NOT_SUPPORTED)
        } else if let Ok(pattern) =
            element.GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId)
        {
            pattern.Toggle().map_err(|_| NOT_SUPPORTED)
        } else if let Ok(pattern) = element
            .GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(UIA_SelectionItemPatternId)
        {
            pattern.Select().map_err(|_| NOT_SUPPORTED)
        } else {
            Err(NOT_SUPPORTED)
        }
    }
}
