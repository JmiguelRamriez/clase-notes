#!/usr/bin/env bash
# Descarga un modelo Whisper al directorio de datos de clase-notes.
# Uso: ./download-whisper-model.sh [medium|small|base|tiny|large]
set -euo pipefail

MODEL="${1:-medium}"
TARGET_DIR="${HOME}/.local/share/clase-notes"
TARGET_FILE="${TARGET_DIR}/ggml-${MODEL}.bin"

REPO="ggerganov/whisper.cpp"
URL="https://huggingface.co/${REPO}/resolve/main/ggml-${MODEL}.bin"

mkdir -p "${TARGET_DIR}"

if [[ -f "${TARGET_FILE}" ]]; then
  echo "El modelo ${MODEL} ya existe en ${TARGET_FILE}"
  exit 0
fi

case "${MODEL}" in
  tiny)   SIZE="~75 MB" ;;
  base)   SIZE="~140 MB" ;;
  small)  SIZE="~460 MB" ;;
  medium) SIZE="~1.5 GB" ;;
  large)  SIZE="~2.9 GB" ;;
  *)      SIZE="?" ;;
esac
echo "Descargando ${MODEL} (${SIZE})..."
echo "  desde: ${URL}"
echo "  hacia: ${TARGET_FILE}"
curl -L --fail -o "${TARGET_FILE}.tmp" "${URL}"
mv "${TARGET_FILE}.tmp" "${TARGET_FILE}"
echo "✓ Modelo descargado: $(du -h "${TARGET_FILE}" | cut -f1)"
