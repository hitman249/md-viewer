#!/bin/sh
# Сборка md-viewer в Docker с последующим возвратом владения файлов
# текущему локальному пользователю (контейнер пишет файлы как root).
set -e
cd "$(dirname "$0")"
HOST_DIR="$(pwd)"
HOST_UID="$(id -u)"
HOST_GID="$(id -g)"

# 1) Сборка приложения + создание .desktop и icon.png рядом с бинарём.
docker run --rm -v "$PWD":/app -w /app md-viewer-build sh -c "
  npm run build &&
  cp /app/icons/icon.png /app/target/release/icon.png &&
  cat > /app/target/release/md-viewer.desktop <<EOF
[Desktop Entry]
Type=Application
Name=md-viewer
Comment=Компактный просмотрщик Markdown
Exec=$HOST_DIR/target/release/md-viewer %F
Icon=$HOST_DIR/target/release/icon.png
Terminal=false
Categories=Utility;
StartupNotify=true
EOF
"

# 2) Возврат владения всем файлам проекта текущему локальному пользователю.
docker run --rm -v "$PWD":/app -w /app md-viewer-build sh -c "chown -R $HOST_UID:$HOST_GID /app"

# 3) AppImage: переключаем с вшитого (несовместимого) webkit из сборочного образа
#    на системный webkit2gtk. Сборочный образ (Debian) пакует webkit, который на
#    целевых системах (Ubuntu/Mesa) падает с "Could not create default EGL display".
#    Поэтому из бандла удаляем webkit-экосистему (webkit + jsc + gst + gtk + glib и
#    прочие зависимости) и оставляем приложению грузить системный libwebkit2gtk.
#    Это стандартная модель Tauri на Linux: требуется системный webkit2gtk.
#    Шаг выполняется на хосте, т.к. список зависимостей берётся из системного webkit.
APPDIR="$HOST_DIR/target/release/bundle/appimage/md-viewer.AppDir"
APPIMAGE=$(ls "$HOST_DIR"/target/release/bundle/appimage/*.AppImage 2>/dev/null | head -n1)
WEBKIT_LIB=/usr/lib/x86_64-linux-gnu/libwebkit2gtk-4.1.so.0
# Временные файлы шага 3 чистим даже при ошибке.
rm_tmp() { rm -f "$HOST_DIR"/target/release/bundle/appimage/runtime.bin \
  "$HOST_DIR"/target/release/bundle/appimage/appimage.sq \
  "$HOST_DIR"/target/release/bundle/appimage/webk_closure.txt \
  "$HOST_DIR"/target/release/bundle/appimage/bundled_libs.txt; }
if [ -n "$APPIMAGE" ] && [ -d "$APPDIR" ] && [ -x "$(command -v mksquashfs)" ] \
  && [ -f "$WEBKIT_LIB" ]; then
  trap rm_tmp EXIT

  # Найти смещение squashfs в AppImage (сканируем магию 'hsqs', проверяем unsquashfs).
  OFFSET=$(python3 - "$APPIMAGE" <<'PY'
import re, subprocess, sys
d = open(sys.argv[1], 'rb').read()
for m in re.finditer(rb'hsqs', d):
    off = m.start()
    r = subprocess.run(['unsquashfs', '-s', '-o', str(off), sys.argv[1]],
                       capture_output=True, text=True)
    if 'Found a valid' in r.stdout:
        print(off)
        break
PY
)
  if [ -z "$OFFSET" ]; then
    echo "Не найден squashfs в AppImage, пропускаю шаг 3."
    exit 0
  fi

  # Извлечь runtime (первые OFFSET байт — он хранит смещение к squashfs).
  dd if="$APPIMAGE" bs=1 count="$OFFSET" of="$HOST_DIR/target/release/bundle/appimage/runtime.bin" 2>/dev/null

  # Список библиотек webkit-экосистемы системного webkit (замыкание ldd).
  ldd "$WEBKIT_LIB" 2>/dev/null \
    | grep -oP '=> [^ ]+\.so[^ ]*' | sed 's/.*\///' | sort -u > "$HOST_DIR/target/release/bundle/appimage/webk_closure.txt"

  ls "$APPDIR/usr/lib/" | grep -E '\.so' | sort -u > "$HOST_DIR/target/release/bundle/appimage/bundled_libs.txt"

  # Удаляем из бандла те библиотеки webkit-экосистемы, что там вшиты.
  while read -r l; do
    if grep -qx "$l" "$HOST_DIR/target/release/bundle/appimage/bundled_libs.txt"; then
      rm -f "$APPDIR/usr/lib/$l"
    fi
  done < "$HOST_DIR/target/release/bundle/appimage/webk_closure.txt"

  # Сам webkit и его вспомогательные процессы тоже убираем.
  rm -f "$APPDIR/usr/lib/libwebkit2gtk-4.1.so.0"
  rm -rf "$APPDIR/usr/lib/x86_64-linux-gnu/webkit2gtk-4.1"

  # Пересобрать squashfs (zstd, 128K — формат AppImage) и склеить с runtime.
  mksquashfs "$APPDIR" "$HOST_DIR/target/release/bundle/appimage/appimage.sq" \
    -comp zstd -b 131072 -processors 4 >/dev/null 2>&1
  cat "$HOST_DIR/target/release/bundle/appimage/runtime.bin" \
      "$HOST_DIR/target/release/bundle/appimage/appimage.sq" > "$APPIMAGE"
  chmod +x "$APPIMAGE"
  echo "AppImage пересобран под системный webkit2gtk: $APPIMAGE"
else
  echo "Пропускаю шаг 3: AppImage/AppDir/mksquashfs/системный webkit2gtk не найдены."
fi
rm_tmp

echo "Готово: сборка выполнена, права файлов возвращены пользователю $(id -un) (uid=$HOST_UID)."
