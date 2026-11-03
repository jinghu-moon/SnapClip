//! Read-only clipboard table. Kit owns virtualization, header and frozen-column geometry.
use crate::PinTable;
use gpui_kit::{
    component::table::{Column, TableDelegate, TableState},
    prelude::*,
    *,
};

fn numeric_column(data: &PinTable, col: usize) -> bool {
    let mut found = false;
    let valid = data
        .rows()
        .iter()
        .skip(usize::from(data.has_header()))
        .all(|row| {
            let cell = row[col].trim();
            if cell.is_empty() {
                return true;
            }
            found = true;
            cell.strip_suffix('%')
                .unwrap_or(cell)
                .parse::<f64>()
                .is_ok_and(f64::is_finite)
        });
    valid && found
}

pub(crate) struct PinTableDelegate {
    data: PinTable,
    widths: Vec<Pixels>,
    numeric: Vec<bool>,
}
impl PinTableDelegate {
    pub(crate) fn new(data: PinTable, window: &Window) -> Self {
        let numeric = (0..data.columns())
            .map(|col| numeric_column(&data, col))
            .collect();
        let widths = (0..data.columns())
            .map(|c| {
                let chars = data
                    .rows()
                    .iter()
                    .map(|r| {
                        r[c].chars()
                            .map(|c| if c.is_ascii() { 1 } else { 2 })
                            .sum::<usize>()
                    })
                    .max()
                    .unwrap_or(8);
                rems((chars as f32 * 0.5 + 2.).clamp(6., 18.)).to_pixels(window.rem_size())
            })
            .collect();
        Self {
            data,
            widths,
            numeric,
        }
    }
    pub(crate) fn has_header(&self) -> bool {
        self.data.has_header()
    }
    pub(crate) fn selected_text(&self, state: &TableState<Self>) -> Option<String> {
        let start = usize::from(self.data.has_header());
        if let Some((row, col)) = state.selected_cell() {
            return self.data.rows().get(row + start)?.get(col).cloned();
        }
        if let Some(row) = state.selected_row() {
            return Some(self.data.rows().get(row + start)?.join("\t"));
        }
        None
    }
}
impl TableDelegate for PinTableDelegate {
    fn columns_count(&self, _: &App) -> usize {
        self.data.columns()
    }
    fn rows_count(&self, _: &App) -> usize {
        self.data.rows().len() - usize::from(self.data.has_header())
    }
    fn column(&self, col: usize, _: &App) -> Column {
        let name = if self.data.has_header() {
            self.data.rows()[0][col].clone()
        } else {
            String::new()
        };
        let column = Column::new(format!("column-{col}"), name)
            .width(self.widths[col])
            .movable(false);
        let column = if col == 0 {
            column.fixed_left()
        } else {
            column
        };
        if self.numeric[col] {
            column.text_right()
        } else {
            column
        }
    }
    fn render_td(
        &mut self,
        row: usize,
        col: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
            .size_full()
            .when(self.numeric[col], |s| s.text_right())
            .child(self.data.rows()[row + usize::from(self.data.has_header())][col].clone())
    }
}

#[cfg(test)]
mod tests {
    use super::numeric_column;
    use crate::PinTable;
    #[test]
    fn only_unambiguous_finite_numbers_use_numeric_alignment() {
        for (values, expected) in [
            (vec!["1", "-2.5", "3e2", ""], true),
            (vec!["25%", "0.5%", ""], true),
            (vec!["NaN", "1"], false),
            (vec!["inf", "1"], false),
            (vec!["", " "], false),
            (vec!["1", "项目2"], false),
            (vec!["1,000", "200"], false),
        ] {
            let table = PinTable::new(
                std::iter::once(vec!["表头".into()])
                    .chain(values.into_iter().map(|v| vec![v.to_string()]))
                    .collect(),
                true,
            )
            .unwrap();
            assert_eq!(numeric_column(&table, 0), expected);
        }
    }
}
