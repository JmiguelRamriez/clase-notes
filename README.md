# clase-notes

> Graba tus clases con el micrófono o tu iPhone, transcribe con **Whisper local**,
> resume con **Ollama** y guarda notas en **Obsidian** — 100% local,
> sin enviar nada a la nube.

```
🎙  clase-notes  ·  Whisper local + Ollama + Obsidian
```

## Características

- 🎤 Captura de audio del micrófono (Linux, macOS, Windows)
- 📱 **iPhone como micrófono inalámbrico** por WiFi (sin instalar nada en el teléfono)
- 🔇 **Filtrado de audio configurable** con presets (Silencio, Normal, Sin filtro)
- 📝 Transcripción local con [Whisper.cpp](https://github.com/ggerganov/whisper.cpp) (vía `whisper-rs`)
- 🧠 Limpieza y resumen con LLM local vía [Ollama](https://ollama.com) (Llama 3, Mistral, Qwen, etc.)
- 📚 Notas Markdown estructuradas con frontmatter, secciones y wikilinks de Obsidian
- 🗂️ Escritura directa a tu bóveda de Obsidian + copia del audio a `attachments/`
- ⌨️ TUI interactiva con `ratatui` para grabar, procesar y ver notas

## Arquitectura

```
Fuente: Micrófono ──► WAV (16kHz mono) ──► Filtro (preset) ──► Whisper
                │                                      │
                │                                      ▼
Fuente: iPhone  ──► WebSocket (WSS) ──► Filtro        LLM (Ollama)
                                              │              │
                                              │   ┌──────────┴──────────┐
                                              │   ▼                     ▼
                                              │  Texto limpio   Notas estructuradas
                                              │   │                     │
                                              └───┴──────────┬──────────┘
                                                             ▼
                                              Markdown con frontmatter
                                                             │
                                                             ▼
                                         Bóveda Obsidian (Clases/<materia>/)
```

## Requisitos

- **Rust 1.75+** (`rustup install stable`)
- **Linux**: `build-essential`, `cmake`, `pkg-config`, `libasound2-dev` (Fedora: `alsa-lib-devel`)
- **macOS**: Xcode Command Line Tools
- **Windows**: MSVC
- **Ollama** instalado y corriendo (`ollama serve`) con un modelo, ej. `ollama pull llama3.1:8b`
- **Modelo Whisper** descargado (ver abajo)
- **GPU (opcional, recomendado para audios >30min)**: NVIDIA RTX 3050 4GB o superior

### Aceleración GPU en Fedora (RTX 3050 4GB)

Para transcribir 1h en ~2min en lugar de 20min:

```bash
# Fedora: driver + CUDA Toolkit (rpmfusion)
sudo dnf install https://mirrors.rpmfusion.org/free/fedora/rpmfusion-free-release-$(rpm -E %fedora).noarch.rpm
sudo dnf install https://mirrors.rpmfusion.org/nonfree/fedora/rpmfusion-nonfree-release-$(rpm -E %fedora).noarch.rpm
sudo dnf install akmod-nvidia xorg-x11-drv-nvidia-cuda cuda-toolkit cmake gcc-c++
# Reiniciar, luego verificar:
nvidia-smi  # debe mostrar RTX 3050

# Compilar con CUDA
cargo build --release --features audio-capture,cuda

## Instalación

```bash
git clone https://github.com/josemr21/clase-notes
cd clase-notes
cargo build --release --features audio-capture
```

El binario queda en `target/release/clase-notes`.

> **Sin tarjeta de sonido / ALSA:** puedes compilar sin la feature
> `audio-capture` (modo `cargo build --release`). El módulo de audio
> estará presente pero solo con un grabador nulo (silencio). Útil
> para CI y para usar el comando `process` con WAVs ya grabados.

## Configuración

Al primer arranque se crea `~/.config/clase-notes/config.toml`:

```toml
[obsidian]
vault_path = "/home/tu_usuario/Obsidian"
notes_subdir = "Clases"

[audio]
sample_rate = 16000
channels = 1

[whisper]
model_path = "/home/tu_usuario/.local/share/clase-notes/ggml-small.bin"
language = "es"
use_gpu = false  # true en RTX 3050 con --features cuda
chunk_secs = 30  # 0 = sin chunked, 30 evita OOM en 4GB para 1h

[llm]
endpoint = "http://localhost:11434"
model = "llama3.1:8b"
temperature = 0.3

[tui]
color_theme = "dark"
```

## Modelo Whisper

Descarga uno de los modelos GGML oficiales. Recomendamos `medium` para español:

```bash
mkdir -p ~/.local/share/clase-notes
cd ~/.local/share/clase-notes

# Modelo tiny (~75 MB) — rápido, el default
curl -L -o ggml-tiny.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin

# Alternativas de mayor calidad:
# curl -L -o ggml-base.bin  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin
# curl -L -o ggml-small.bin https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin
# curl -L -o ggml-medium.bin https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin
```

## Uso

### TUI interactiva

```bash
clase-notes
```

Navegación:

```
┌─ clase-notes ─────────────────────────────────┐
│  Grabar clase                                │
│  Procesar WAV existente                      │
│  Ver notas recientes                         │
│  Salir                                       │
└───────────────────────────────────────────────┘
```

En la pantalla de grabación:
- Tab para cambiar entre Materia / Tema / Tags
- Enter para empezar a grabar
- Espacio = pausa, Enter = detener, Esc = cancelar

### CLI directa

Grabar y procesar en un solo paso (no interactivo):

```bash
clase-notes record \
  --materia "Cálculo II" \
  --tema "Límites laterales" \
  --tags calculo-ii,limites
```

Procesar un WAV ya existente:

```bash
clase-notes process ~/grabaciones/clase.wav \
  --materia "Cálculo II" \
  --tema "Límites laterales" \
  --tags calculo-ii,limites
```

Verificar entorno:

```bash
clase-notes doctor
```

## Estructura de la nota generada

```markdown
---
title: "Cálculo II — Límites laterales"
subject: "Cálculo II"
topic: "Límites laterales"
date: 2026-09-01
duration: "47:12"
tags: [clase, calculo-ii, limites]
audio: "[[2026-09-01-calculo-ii-limites-laterales.wav]]"
---

# Cálculo II — Límites laterales

**Fecha:** 2026-09-01  
**Materia:** Cálculo II  
**Tema:** Límites laterales  
**Duración:** 47:12

## Resumen

...

## Puntos clave

- ...

## Conceptos

- [[Límite]] — valor al que tiende una función
- [[Continuidad]] — no hay saltos

## Tareas / pendientes

- [ ] ...

---

## Transcripción completa

> ...

---

*Generado por `clase-notes` · Whisper + Ollama*
```

## Detalles técnicos

### Pipeline de procesamiento

1. **Captura** (`cpal` o WebSocket del iPhone) → WAV 16 kHz mono PCM 16-bit
2. **Filtrado** (noise gate + RMS normalizer según preset) → audio limpio
3. **Transcripción** (`whisper-rs` con modelo `ggml-medium`) → texto crudo
4. **Limpieza** (LLM, temperatura 0.2) → texto sin muletillas ni errores
5. **Resumen** (LLM, temperatura 0.3) → 4 secciones en Markdown
6. **Generación** de nota con frontmatter YAML
7. **Copia** del audio a `vault/Clases/attachments/`
8. **Escritura** de la nota en `vault/Clases/`

### iPhone como micrófono inalámbrico

`clase-notes` puede usar tu iPhone como micrófono remoto por WiFi. Útil cuando la PC está lejos del profesor y vos estás cerca con el teléfono.

**Cómo funciona:**

1. Abrí la TUI y elegí **"Conectar teléfono"** en el menú.
2. La TUI muestra un **QR code** con la URL `https://<ip-local>:8443/`.
3. Escaneá el QR con la cámara del iPhone.
4. Safari se abre. Te muestra un aviso de certificado (es autofirmado, generado en tu PC): tocá **"Opciones" → "Visitar este sitio web"**.
5. Tocá el botón **"Iniciar"** y aceptá el permiso de micrófono.
6. La TUI muestra "iPhone conectado ✓".

Una vez conectado, andá a **"Grabar clase"** y elegí **"Fuente: Teléfono"**. El audio se transmite en tiempo real al servidor local (WebSocket seguro, 20 ms de latencia).

**Importante:**

- El iPhone y la PC deben estar en la **misma red WiFi**.
- iOS suspende Safari si lo dejás en background. Mantené la pantalla prendida.
- El certificado TLS es autofirmado (se regenera una vez y se cachea en `~/.local/share/clase-notes/recordings/phone-server.pem`).

### Filtrado de audio con presets

Al grabar, podés elegir un **preset de filtrado** desde la TUI (con `Tab` llegás al campo `Filtro` y con `←/→` rotás):

| Preset | Noise gate | Normalizer | Cuándo usarlo |
|--------|-----------|-----------|---------------|
| **Silencio** | -30 dB | fuerte (RMS 0.3) | Salón ruidoso, compañeros hablando fuerte |
| **Normal** | -40 dB | suave (RMS 0.5) | Uso general |
| **Sin filtro** | off | off | Audio crudo del micrófono (cuando ya hay poco ruido) |

El filtrado se aplica **después** de la captura, en post-procesamiento del WAV. Funciona igual con micrófono local o iPhone.

> El navegador del iPhone ya aplica `echoCancellation`, `noiseSuppression` y `autoGainControl` nativos. El preset de `clase-notes` es una segunda capa para ruido que el browser no alcanza a filtrar.

### Recursos

- **RAM:** ~1.5 GB para Whisper medium + ~5 GB para Llama 3.1 8B (CPU)
- **CPU:** 4-8 cores recomendados; el primer build de `whisper.cpp` tarda varios minutos
- **Disco:** ~3 GB para modelos + notas

## Limitaciones conocidas

- La calidad de la transcripción depende del modelo Whisper y de la calidad del micrófono. Para clases magistrales largas, considera un micrófono de solapa, un dictáfono cercano, o usar el iPhone cerca del profesor.
- El LLM a veces invierte el orden de los puntos. Inspecciona las notas antes de confiar ciegamente.
- Sin diarización (quién habla cuándo); todo el audio se trata como una sola voz.
- iOS puede suspender Safari en background; mantené la pantalla del iPhone prendida durante la grabación.
- El filtrado de audio es básico (noise gate + RMS normalizer). Para ambientes muy ruidosos, un micrófono direccional sigue siendo mejor que cualquier filtro.

## Licencia

MIT
