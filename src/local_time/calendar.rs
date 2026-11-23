use chrono::{Datelike, Days, Months, NaiveDate};
use slint::{ModelRc, VecModel};

use crate::{AmicaWindow, CalendarDay};

pub(super) struct Calendar {
    today: NaiveDate,
    month: NaiveDate,
}

impl Calendar {
    pub fn new(today: NaiveDate) -> Self {
        Self {
            today,
            month: first_of_month(today),
        }
    }

    pub fn change_month(&mut self, offset: i32) -> bool {
        let months = Months::new(offset.unsigned_abs());
        let month = if offset < 0 {
            self.month.checked_sub_months(months)
        } else {
            self.month.checked_add_months(months)
        };
        let Some(month) = month else { return false };
        self.month = month;
        true
    }

    pub fn update_today(&mut self, today: NaiveDate) -> bool {
        if self.today == today {
            return false;
        }
        if self.month == first_of_month(self.today) {
            self.month = first_of_month(today);
        }
        self.today = today;
        true
    }

    pub fn publish(&self, window: &AmicaWindow) {
        window.set_calendar_month(self.month.format("%B %Y").to_string().into());
        window.set_calendar_days(ModelRc::new(VecModel::from(self.days())));
    }

    fn days(&self) -> Vec<CalendarDay> {
        let leading = u64::from(self.month.weekday().num_days_from_sunday());
        (0_u64..42)
            .map(|index| {
                let date = index
                    .checked_sub(leading)
                    .and_then(|offset| self.month.checked_add_days(Days::new(offset)))
                    .filter(|date| date.month() == self.month.month());
                CalendarDay {
                    label: date.map_or_else(Default::default, |date| date.day().to_string().into()),
                    is_today: date == Some(self.today),
                }
            })
            .collect()
    }
}

fn first_of_month(date: NaiveDate) -> NaiveDate {
    date.with_day(1).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_leap_day_and_updates_the_current_month_at_midnight() {
        let february = NaiveDate::from_ymd_opt(2024, 2, 29).unwrap();
        let mut calendar = Calendar::new(february);
        let days = calendar.days();
        assert!(days[..4].iter().all(|day| day.label.is_empty()));
        assert_eq!(days[4].label, "1");
        assert_eq!(days[32].label, "29");
        assert!(days[32].is_today);
        assert!(days[33].label.is_empty());

        assert!(calendar.update_today(february.succ_opt().unwrap()));
        assert_eq!(calendar.month.month(), 3);
        assert!(calendar.days()[5].is_today);
        assert!(calendar.change_month(-1));
        assert_eq!(calendar.month.month(), 2);
        assert!(calendar.days().iter().all(|day| !day.is_today));
    }
}
