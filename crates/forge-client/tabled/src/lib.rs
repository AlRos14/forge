#![forbid(unsafe_code)]

use std::fmt;

pub struct Table {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
}

impl Table {
    pub fn from_rows(headers: &[&str], rows: Vec<Vec<String>>) -> Self {
        Self {
            headers: headers.iter().map(|value| (*value).to_owned()).collect(),
            rows,
        }
    }
}

impl fmt::Display for Table {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let widths = self.widths();
        write_separator(formatter, &widths)?;
        write_row(formatter, &self.headers, &widths)?;
        write_separator(formatter, &widths)?;
        for row in &self.rows {
            write_row(formatter, row, &widths)?;
        }
        write_separator(formatter, &widths)
    }
}

impl Table {
    fn widths(&self) -> Vec<usize> {
        let mut widths = self
            .headers
            .iter()
            .map(|value| value.chars().count())
            .collect::<Vec<_>>();

        for row in &self.rows {
            for (index, cell) in row.iter().enumerate() {
                if let Some(width) = widths.get_mut(index) {
                    *width = (*width).max(cell.chars().count());
                }
            }
        }

        widths
    }
}

fn write_separator(formatter: &mut fmt::Formatter<'_>, widths: &[usize]) -> fmt::Result {
    formatter.write_str("+")?;
    for width in widths {
        formatter.write_str(&"-".repeat(width + 2))?;
        formatter.write_str("+")?;
    }
    writeln!(formatter)
}

fn write_row(formatter: &mut fmt::Formatter<'_>, row: &[String], widths: &[usize]) -> fmt::Result {
    formatter.write_str("|")?;
    for (index, width) in widths.iter().enumerate() {
        let value = row.get(index).map(String::as_str).unwrap_or("");
        write!(formatter, " {value:<width$} |")?;
    }
    writeln!(formatter)
}
