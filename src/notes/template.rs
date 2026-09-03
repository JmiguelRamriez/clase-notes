//! Plantilla Markdown por defecto para notas de clase.

/// Plantilla usada por `NoteBuilder::default_template`. Usa
/// `{title}`, `{date}`, `{materia}`, `{tema}`, `{duration}`,
/// `{summary}`, `{key_points}`, `{concepts}`, `{transcript}`,
/// `{tasks}` como placeholders.
pub const DEFAULT_TEMPLATE: &str = "\
---
title: \"{title}\"
subject: \"{materia}\"
topic: \"{tema}\"
date: {date}
duration: \"{duration}\"
tags: [clase, {tags}]
audio: \"[[{audio_link}]]\"
---

# {title}

**Fecha:** {date}  
**Materia:** {materia}  
**Tema:** {tema}  
**Duración:** {duration}

## Resumen

{summary}

## Puntos clave

{key_points}

## Conceptos

{concepts}

## Tareas / pendientes

{tasks}

---

## Transcripción completa

{transcript}

---

*Generado por `clase-notes` · [Whisper](https://github.com/ggerganov/whisper.cpp) + Ollama*
";
