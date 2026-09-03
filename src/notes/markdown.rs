//! `NoteBuilder` toma los datos de una clase y genera el Markdown final
//! aplicando la plantilla, escapando caracteres conflictivos y
//! formateando listas.

use anyhow::{Context, Result};
use chrono::NaiveDate;
use std::path::Path;

use crate::llm::SummaryOutput;
use crate::notes::template::DEFAULT_TEMPLATE;

/// Datos de entrada para construir la nota.
pub struct NoteInput<'a> {
    pub materia: &'a str,
    pub tema: &'a str,
    pub date: NaiveDate,
    pub duration_secs: f32,
    pub transcript: &'a str,
    pub summary: &'a SummaryOutput,
    pub audio_link: &'a str,
    pub tags: &'a [String],
}

pub struct NoteBuilder {
    template: String,
}

impl Default for NoteBuilder {
    fn default() -> Self {
        Self {
            template: DEFAULT_TEMPLATE.to_string(),
        }
    }
}

impl NoteBuilder {
    #[allow(dead_code)]
    pub fn with_template(template: String) -> Self {
        Self { template }
    }

    pub fn build(&self, input: &NoteInput) -> Result<String> {
        let title = format!("{} — {}", input.materia, input.tema);
        let duration = format_duration(input.duration_secs);
        let tags_csv = input
            .tags
            .iter()
            .map(|t| t.as_str())
            .collect::<Vec<_>>()
            .join(", ");

        let key_points = render_bullets(&input.summary.puntos_clave);
        let concepts = render_concepts(&input.summary.conceptos);
        let tasks = render_tasks(&input.summary.tareas);
        let transcript = indent_blockquote(input.transcript);

        let summary = if input.summary.resumen.is_empty() {
            "_Sin resumen (LLM no devolvió contenido)._".to_string()
        } else {
            input.summary.resumen.clone()
        };

        let rendered = self
            .template
            .replace("{title}", &yaml_escape(&title))
            .replace("{date}", &input.date.to_string())
            .replace("{materia}", &yaml_escape(input.materia))
            .replace("{tema}", &yaml_escape(input.tema))
            .replace("{duration}", &duration)
            .replace("{tags}", &tags_csv)
            .replace("{audio_link}", &sanitize_link(input.audio_link))
            .replace("{summary}", &summary)
            .replace("{key_points}", &key_points)
            .replace("{concepts}", &concepts)
            .replace("{tasks}", &tasks)
            .replace("{transcript}", &transcript);

        // Validación: la plantilla debe haber consumido todos los placeholders.
        if rendered.contains("{title}")
            || rendered.contains("{date}")
            || rendered.contains("{materia}")
        {
            anyhow::bail!("la plantilla no consumió todos los placeholders");
        }
        Ok(rendered)
    }
}

fn format_duration(secs: f32) -> String {
    let total = secs.round() as u32;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{}:{:02}:{:02}", h, m, s)
    } else {
        format!("{}:{:02}", m, s)
    }
}

fn render_bullets(items: &[String]) -> String {
    if items.is_empty() {
        return "_Sin puntos clave._".to_string();
    }
    items
        .iter()
        .map(|i| format!("- {}", i))
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_concepts(items: &[(String, String)]) -> String {
    if items.is_empty() {
        return "_Sin conceptos._".to_string();
    }
    items
        .iter()
        .map(|(name, def)| {
            if def.is_empty() {
                format!("- [[{}]]", name)
            } else {
                format!("- [[{}]] — {}", name, def)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_tasks(items: &[String]) -> String {
    if items.is_empty() {
        return "_Ninguna explícita._".to_string();
    }
    items
        .iter()
        .map(|t| format!("- [ ] {}", t))
        .collect::<Vec<_>>()
        .join("\n")
}

fn indent_blockquote(s: &str) -> String {
    s.lines()
        .map(|l| {
            if l.is_empty() {
                ">".to_string()
            } else {
                format!("> {}", l)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn yaml_escape(s: &str) -> String {
    // Para valores entre comillas dobles YAML, escapamos backslash y ".
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn sanitize_link(s: &str) -> String {
    s.replace('"', "").replace('\n', " ")
}

/// Escribe la nota al directorio `dest_dir/<archivo>.md`. Si el archivo
/// ya existe, añade sufijo `-1`, `-2`, etc.
pub fn write_note(
    dest_dir: &Path,
    materia: &str,
    tema: &str,
    date: NaiveDate,
    content: &str,
) -> Result<std::path::PathBuf> {
    std::fs::create_dir_all(dest_dir)
        .with_context(|| format!("creando {}", dest_dir.display()))?;
    let base = slugify(&format!("{}-{}-{}", date, materia, tema));
    let mut candidate = dest_dir.join(format!("{}.md", base));
    let mut n = 1;
    while candidate.exists() {
        candidate = dest_dir.join(format!("{}-{}.md", base, n));
        n += 1;
    }
    std::fs::write(&candidate, content)
        .with_context(|| format!("escribiendo {}", candidate.display()))?;
    Ok(candidate)
}

pub fn slugify(s: &str) -> String {
    use unicode_normalization::UnicodeNormalization;

    // NFD descompone caracteres con diacríticos; luego filtramos las marcas.
    let normalized: String = s
        .nfd()
        .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
        .collect();

    normalized
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> NoteInput<'static> {
        static TAGS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
        let tags = TAGS.get_or_init(|| {
            vec!["calculo-ii".to_string(), "limites".to_string()]
        });
        static SUMMARY: std::sync::OnceLock<SummaryOutput> = std::sync::OnceLock::new();
        let summary = SUMMARY.get_or_init(|| SummaryOutput {
            resumen: "Introducción a límites".into(),
            puntos_clave: vec!["Definición épsilon-delta".into(), "Límites laterales".into()],
            conceptos: vec![("Límite".into(), "valor al que tiende".into())],
            tareas: vec!["Ejercicios 1-8".into()],
        });
        NoteInput {
            materia: "Cálculo II",
            tema: "Límites",
            date: NaiveDate::from_ymd_opt(2026, 9, 1).unwrap(),
            duration_secs: 2832.0,
            transcript: "hoy vimos límites. el límite de f cuando x tiende a 0...",
            summary,
            audio_link: "2026-09-01-calculo-ii-limites.wav",
            tags,
        }
    }

    #[test]
    fn build_contains_required_sections() {
        let nb = NoteBuilder::default();
        let md = nb.build(&fixture()).unwrap();
        assert!(md.contains("## Resumen"));
        assert!(md.contains("## Puntos clave"));
        assert!(md.contains("## Conceptos"));
        assert!(md.contains("## Tareas / pendientes"));
        assert!(md.contains("## Transcripción completa"));
        assert!(md.contains("Cálculo II"));
    }

    #[test]
    fn build_renders_wikilinks() {
        let nb = NoteBuilder::default();
        let md = nb.build(&fixture()).unwrap();
        assert!(md.contains("[[Límite]]"));
    }

    #[test]
    fn duration_formats_as_hh_mm_ss() {
        let nb = NoteBuilder::default();
        let md = nb.build(&fixture()).unwrap();
        assert!(md.contains("47:12"));
    }

    #[test]
    fn slugify_lowercases_and_dashes() {
        assert_eq!(slugify("Cálculo II: Límites!"), "calculo-ii-limites");
    }

    #[test]
    fn empty_sections_show_placeholder() {
        let mut f = fixture();
        static EMPTY: std::sync::OnceLock<SummaryOutput> = std::sync::OnceLock::new();
        let empty = EMPTY.get_or_init(|| SummaryOutput {
            resumen: "".into(),
            puntos_clave: vec![],
            conceptos: vec![],
            tareas: vec![],
        });
        f.summary = empty;
        let md = NoteBuilder::default().build(&f).unwrap();
        assert!(md.contains("_Sin resumen"));
        assert!(md.contains("_Sin puntos clave"));
        assert!(md.contains("_Sin conceptos"));
        assert!(md.contains("_Ninguna explícita"));
    }
}
