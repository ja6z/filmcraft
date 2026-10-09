//! Interface translations. Command ids, document text and file names remain stable.
//! Untranslated labels fall back to English so coverage can grow incrementally.
//!
//! Japanese text uses the Japanese craft-fonts when FilmCraft was built with them (`CRAFT_FONTS_DIR`;
//! `theme::install` already puts them in every font family, see [`craft_japanese_font`]), otherwise
//! a font already installed on the system ([`system_japanese_font`]); with neither, switching to
//! Japanese is refused with a message.

use std::sync::{Arc, OnceLock};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    En,
    Ja,
    Es,
}

impl Language {
    pub const ALL: [Self; 3] = [Self::En, Self::Ja, Self::Es];

    pub fn name(self) -> &'static str {
        match self {
            Self::En => "English",
            Self::Ja => "日本語",
            Self::Es => "Español",
        }
    }

    pub fn parse(code: &str) -> Option<Self> {
        match code {
            "en" => Some(Self::En),
            "ja" => Some(Self::Ja),
            "es" => Some(Self::Es),
            _ => None,
        }
    }

    pub fn tr(self, text: &str) -> &str {
        let table = match self {
            Self::En => return text,
            Self::Ja => JAPANESE,
            Self::Es => SPANISH,
        };
        table.iter().find(|(english, _)| *english == text).map_or(text, |(_, translated)| translated)
    }
}

/// Text every Japanese interface font must cover (menus use kanji, hiragana and katakana).
const JAPANESE_SAMPLE: &str = "日本語ファイル編集あア";

/// Installed families preferred for Japanese interface text, best first (Gothic / sans-serif faces
/// read best at menu sizes). Any other installed face that covers [`JAPANESE_SAMPLE`] is used if
/// none of these is present.
const PREFERRED_JAPANESE: &[&str] = &[
    "Hiragino Sans",
    "Hiragino Kaku Gothic ProN",
    "Hiragino Kaku Gothic Pro",
    "Yu Gothic UI",
    "Yu Gothic",
    "Meiryo UI",
    "Meiryo",
    "Noto Sans CJK JP",
    "Noto Sans JP",
    "Source Han Sans JP",
    "Source Han Sans",
    "IPAexGothic",
    "IPAGothic",
    "TakaoGothic",
    "VL Gothic",
];

const JAPANESE_FONT: &str = "system-japanese";

/// A Japanese font already installed on this system, for the interface (none is bundled). Looked up
/// once per process: the system font folders are scanned on first use (name tables only), then the
/// chosen face's file is read. `None` on the web and on systems without a Japanese font.
pub fn system_japanese_font() -> Option<Arc<egui::FontData>> {
    static FONT: OnceLock<Option<Arc<egui::FontData>>> = OnceLock::new();
    FONT.get_or_init(|| {
        filmcraft_text::fonts::scan_system();
        let faces: Vec<_> = filmcraft_text::fonts::all_faces().into_iter().filter(|f| f.info.origin == "system" && !f.info.italic).collect();
        let covers = |f: &filmcraft_text::fonts::Face| JAPANESE_SAMPLE.chars().all(|c| f.has_char(c));
        // within a family, the face closest to regular weight
        let by_weight = |f: &&Arc<filmcraft_text::fonts::Face>| f.info.weight.abs_diff(400);
        let preferred =
            PREFERRED_JAPANESE.iter().find_map(|name| faces.iter().filter(|f| f.info.family.eq_ignore_ascii_case(name) && covers(f)).min_by_key(by_weight));
        let face = preferred.or_else(|| faces.iter().filter(|f| covers(f)).min_by_key(by_weight))?;
        // kept for the life of the process (one font, read once) so installing it again after a
        // theme change shares the bytes instead of copying the whole file
        let bytes: &'static [u8] = Box::leak(face.data()?.into_boxed_slice());
        Some(Arc::new(egui::FontData { font: std::borrow::Cow::Borrowed(bytes), index: face.info.index, tweak: Default::default() }))
    })
    .clone()
}

/// Whether the craft-fonts build input (empty unless built with `CRAFT_FONTS_DIR`) supplies a
/// Japanese interface font: some craft-fonts face covers [`JAPANESE_SAMPLE`]. `theme::install` adds
/// these faces to every font family, so nothing else needs installing (and no system scan runs).
pub fn craft_japanese_font() -> bool {
    filmcraft_text::fonts::craft_japanese().next().is_some()
        && filmcraft_text::fonts::all_faces()
            .iter()
            .any(|f| f.info.origin == filmcraft_text::fonts::CRAFT_ORIGIN && JAPANESE_SAMPLE.chars().all(|c| f.has_char(c)))
}

/// Japanese for the interface: true when built with the craft-fonts (already installed by
/// `theme::install`). Otherwise add the system's Japanese font as the last fallback of every theme font family, from the next
/// pass on. Returns false (and changes nothing) when no Japanese font is installed. Call it again
/// after `theme::install`, which replaces the font definitions.
pub fn install_japanese_font(ctx: &egui::Context) -> bool {
    if craft_japanese_font() {
        return true;
    }
    let Some(font) = system_japanese_font() else { return false };
    let families = crate::theme::font_families()
        .into_iter()
        .map(|family| egui::epaint::text::InsertFontFamily { family, priority: egui::epaint::text::FontPriority::Lowest })
        .collect();
    // queued for the next pass (works before the first frame); a no-op when already installed
    ctx.add_font(egui::epaint::text::FontInsert { name: JAPANESE_FONT.into(), data: (*font).clone(), families });
    true
}

const SPANISH: &[(&str, &str)] = &[
    ("About FilmCraft", "Acerca de FilmCraft"),
    ("Add Audio Submix Track", "Añadir pista de submezcla de audio"),
    ("Add Caption at Playhead", "Añadir subtítulo en el cabezal"),
    ("Add Chapter Marker…", "Añadir marcador de capítulo…"),
    ("Add Beat Markers…", "Añadir marcadores de beat…"),
    ("Add Edit", "Añadir edición"),
    ("Add Edit to All Tracks", "Añadir edición a todas las pistas"),
    ("Add Flash Cue Marker…", "Añadir marcador de referencia Flash…"),
    ("Add Frame Hold", "Añadir fotograma congelado"),
    ("Add Guide…", "Añadir guía…"),
    ("Add Marker", "Añadir marcador"),
    ("Add New Caption Track…", "Añadir nueva pista de subtítulos…"),
    ("Add Range Marker", "Añadir marcador de rango"),
    ("Add Range Marker to In and Out", "Añadir marcador de rango de entrada a salida"),
    ("Add Tracks…", "Añadir pistas…"),
    ("Adjustment Layer…", "Capa de ajuste…"),
    ("Align to Selection", "Alinear con la selección"),
    ("Align to Video Frame", "Alinear con el fotograma de video"),
    ("Align to Video Frame as Group", "Alinear con el fotograma de video como grupo"),
    ("All Panels", "Todos los paneles"),
    ("Alpha", "Alfa"),
    ("Appearance", "Apariencia"),
    ("Appearance…", "Apariencia…"),
    ("Apply Audio Transition", "Aplicar transición de audio"),
    ("Apply Default Transitions to Selection", "Aplicar transiciones predeterminadas a la selección"),
    ("Apply Video Transition", "Aplicar transición de video"),
    ("Arrange", "Organizar"),
    ("ArtCraft Website", "Sitio web de ArtCraft"),
    ("Assembly", "Ensamblaje"),
    ("Attach Proxies…", "Adjuntar proxies…"),
    ("Audio Channels…", "Canales de audio…"),
    ("Audio Clip Mixer", "Mezclador de clips de audio"),
    ("Audio Gain…", "Ganancia de audio…"),
    ("Audio Hardware…", "Hardware de audio…"),
    ("Audio In", "Entrada de audio"),
    ("Audio Meters", "Medidores de audio"),
    ("Audio Options", "Opciones de audio"),
    ("Audio Out", "Salida de audio"),
    ("Audio Track Mixer", "Mezclador de pistas de audio"),
    ("Audio Waveform", "Forma de onda de audio"),
    ("Auto Save…", "Guardado automático…"),
    ("Automate to Sequence…", "Automatizar a secuencia…"),
    ("Bars and Tone…", "Barras y tono…"),
    ("Bin", "Bandeja"),
    ("Bin From Selection", "Bandeja a partir de la selección"),
    ("Black Video…", "Video negro…"),
    ("Blue", "Azul"),
    ("Bottom", "Abajo"),
    ("Breakout to Mono", "Separar en mono"),
    ("Bring Forward", "Traer adelante"),
    ("Bring to Front", "Traer al frente"),
    ("Brown", "Marrón"),
    ("Captions", "Subtítulos"),
    ("Captions and Graphics", "Subtítulos y gráficos"),
    ("Captions…", "Subtítulos…"),
    ("Caribbean", "Caribe"),
    ("Center Horizontally", "Centrar horizontalmente"),
    ("Center Vertically", "Centrar verticalmente"),
    ("Cerulean", "Cerúleo"),
    ("Clear", "Borrar"),
    ("Clear Guides", "Borrar guías"),
    ("Clear In", "Borrar entrada"),
    ("Clear In and Out", "Borrar entrada y salida"),
    ("Clear Markers", "Borrar marcadores"),
    ("Clear Out", "Borrar salida"),
    ("Clear Selected Marker", "Borrar marcador seleccionado"),
    ("Close", "Cerrar"),
    ("Close All Other Projects", "Cerrar todos los demás proyectos"),
    ("Close All Projects", "Cerrar todos los proyectos"),
    ("Close Gap", "Cerrar hueco"),
    ("Close Project", "Cerrar proyecto"),
    ("Color Management…", "Gestión del color…"),
    ("Color Matte…", "Mate de color…"),
    ("Comparison View", "Vista de comparación"),
    ("Composite Video", "Video compuesto"),
    ("Consolidate Duplicates", "Consolidar duplicados"),
    ("Copy", "Copiar"),
    ("Copy Paste Includes Sequence Markers", "Copiar y pegar incluye marcadores de secuencia"),
    ("Create Captions from Transcript…", "Crear subtítulos a partir de la transcripción…"),
    ("Create Multi-Camera Source Sequence…", "Crear secuencia de origen multicámara…"),
    ("Create Proxies…", "Crear proxies…"),
    ("Cut", "Cortar"),
    ("Darkest", "Más oscuro"),
    ("Delete Render Files", "Eliminar archivos de procesamiento"),
    ("Delete Render Files In to Out", "Eliminar archivos de procesamiento de entrada a salida"),
    ("Delete Tracks…", "Eliminar pistas…"),
    ("Delete Transcript", "Eliminar transcripción"),
    ("Demo Footage", "Material de demostración"),
    ("Demo Project", "Proyecto de demostración"),
    ("Deselect All", "Deseleccionar todo"),
    ("Detach Proxies", "Separar proxies"),
    ("Display Mode", "Modo de visualización"),
    ("Distribute", "Distribuir"),
    ("Distribute Horizontally", "Distribuir horizontalmente"),
    ("Distribute Space Horizontally", "Distribuir espacio horizontalmente"),
    ("Distribute Space Vertically", "Distribuir espacio verticalmente"),
    ("Distribute Vertically", "Distribuir verticalmente"),
    ("Duplicate", "Duplicar"),
    ("Dynamic Audio Waveforms", "Formas de onda de audio dinámicas"),
    ("Edit", "Edición"),
    ("Edit Offline…", "Editar sin conexión…"),
    ("Edit Original", "Editar original"),
    ("Edit Subclip…", "Editar subclip…"),
    ("Editing", "Edición"),
    ("Effect Controls", "Controles de efectos"),
    ("Effects", "Efectos"),
    ("Ellipse", "Elipse"),
    ("Enable", "Activar"),
    ("Enable Remix", "Activar remezcla"),
    ("Essential Graphics", "Gráficos esenciales"),
    ("Essential Sound", "Sonido esencial"),
    ("Events", "Eventos"),
    ("Export", "Exportar"),
    ("Export As Motion Graphics Template…", "Exportar como plantilla de animación…"),
    ("Extract", "Extraer"),
    ("Extract Audio", "Extraer audio"),
    ("Field Options…", "Opciones de campo…"),
    ("File", "Archivo"),
    ("File…", "Archivo…"),
    ("Fill frame", "Rellenar fotograma"),
    ("FilmCraft Help…", "Ayuda de FilmCraft…"),
    ("FilmCraft on GitHub", "FilmCraft en GitHub"),
    ("FilmCraft on getartcraft.com", "FilmCraft en getartcraft.com"),
    ("Find Next", "Buscar siguiente"),
    ("Find…", "Buscar…"),
    ("Fit", "Ajustar"),
    ("Fit to frame", "Ajustar al fotograma"),
    ("Flatten", "Acoplar"),
    ("Forest", "Bosque"),
    ("Frame Blending", "Fusión de fotogramas"),
    ("Frame Hold Options…", "Opciones de fotograma congelado…"),
    ("Frame Sampling", "Muestreo de fotogramas"),
    ("From Bin", "Desde la bandeja"),
    ("From Source Monitor", "Desde el monitor de origen"),
    ("From Source Monitor, Match Frame", "Desde el monitor de origen, fotograma coincidente"),
    ("From file…", "Desde archivo…"),
    ("Full", "Completa"),
    ("Generate Audio Waveform", "Generar forma de onda de audio"),
    ("Get Media File Properties for", "Obtener propiedades del archivo multimedia de"),
    ("Go to Gap", "Ir al hueco"),
    ("Go to In", "Ir a la entrada"),
    ("Go to Next Caption Segment", "Ir al siguiente segmento de subtítulo"),
    ("Go to Next Marker", "Ir al siguiente marcador"),
    ("Go to Out", "Ir a la salida"),
    ("Go to Previous Caption Segment", "Ir al segmento de subtítulo anterior"),
    ("Go to Previous Marker", "Ir al marcador anterior"),
    ("Go to Split", "Ir a la división"),
    ("Graphics and Titles", "Gráficos y títulos"),
    ("Graphics…", "Gráficos…"),
    ("Green", "Verde"),
    ("Group", "Agrupar"),
    ("Guide Templates", "Plantillas de guías"),
    ("Help", "Ayuda"),
    ("Hide All Caption Tracks", "Ocultar todas las pistas de subtítulos"),
    ("High Quality Playback", "Reproducción de alta calidad"),
    ("History", "Historial"),
    ("Import From", "Importar desde"),
    ("Import Image Sequence…", "Importar secuencia de imágenes…"),
    ("Import from Media Browser", "Importar desde el navegador de medios"),
    ("Import…", "Importar…"),
    ("Info", "Información"),
    ("Ingest Settings…", "Ajustes de ingesta…"),
    ("Insert", "Insertar"),
    ("Insert Frame Hold Segment", "Insertar segmento de fotograma congelado"),
    ("Install Motion Graphics Template…", "Instalar plantilla de animación…"),
    ("Interpret Footage…", "Interpretar material…"),
    ("Join the ArtCraft Discord…", "Únete al Discord de ArtCraft…"),
    ("Keyboard Shortcuts…", "Atajos de teclado…"),
    ("Label", "Etiqueta"),
    ("Labels…", "Etiquetas…"),
    ("Language", "Idioma"),
    ("Lavender", "Lavanda"),
    ("Learning", "Aprendizaje"),
    ("Left", "Izquierda"),
    ("Libraries", "Bibliotecas"),
    ("Lift", "Levantar"),
    ("Light", "Claro"),
    ("Link", "Vincular"),
    ("Link Media…", "Vincular medios…"),
    ("Linked Selection", "Selección vinculada"),
    ("Lock Guides", "Bloquear guías"),
    ("Lumetri Color", "Color Lumetri"),
    ("Lumetri Scopes", "Ámbitos Lumetri"),
    ("Magnification", "Ampliación"),
    ("Make Offline…", "Desconectar…"),
    ("Make Subclip…", "Crear subclip…"),
    ("Make Subsequence", "Crear subsecuencia"),
    ("Manage Guides…", "Gestionar guías…"),
    ("Mark Clip", "Marcar clip"),
    ("Mark In", "Marcar entrada"),
    ("Mark Out", "Marcar salida"),
    ("Mark Selection", "Marcar selección"),
    ("Mark Split", "Marcar división"),
    ("Markers", "Marcadores"),
    ("Match Frame", "Fotograma coincidente"),
    ("Media Analysis & Transcription…", "Análisis y transcripción de medios…"),
    ("Media Browser", "Navegador de medios"),
    ("Media Cache…", "Caché de medios…"),
    ("Media…", "Medios…"),
    ("Medium", "Medio"),
    ("Memory…", "Memoria…"),
    ("Merge Clips…", "Combinar clips…"),
    ("Metadata", "Metadatos"),
    ("Modify", "Modificar"),
    ("Motion Graphics Template…", "Plantilla de animación…"),
    ("Multi-Camera", "Multicámara"),
    ("Multi-Camera View", "Vista multicámara"),
    ("Nest…", "Anidar…"),
    ("New", "Nuevo"),
    ("New Layer", "Nueva capa"),
    ("New Project Panel", "Nuevo panel de proyecto"),
    ("Next in Sequence", "Siguiente en la secuencia"),
    ("Next in Track", "Siguiente en la pista"),
    ("Normalize Mix Track…", "Normalizar pista de mezcla…"),
    ("Offline File…", "Archivo sin conexión…"),
    ("Open Project…", "Abrir proyecto…"),
    ("Optical Flow", "Flujo óptico"),
    ("Overwrite", "Sobrescribir"),
    ("Paste", "Pegar"),
    ("Paste Attributes…", "Pegar atributos…"),
    ("Paste Insert", "Pegar e insertar"),
    ("Paused Resolution", "Resolución en pausa"),
    ("Playback Resolution", "Resolución de reproducción"),
    ("Playback…", "Reproducción…"),
    ("Plugins…", "Complementos…"),
    ("Polygon", "Polígono"),
    ("Preferences", "Preferencias"),
    ("Previous in Sequence", "Anterior en la secuencia"),
    ("Previous in Track", "Anterior en la pista"),
    ("Program", "Programa"),
    ("Progress", "Progreso"),
    ("Project", "Proyecto"),
    ("Project Manager…", "Administrador de proyectos…"),
    ("Project Settings", "Ajustes del proyecto"),
    ("Project…", "Proyecto…"),
    ("Properties", "Propiedades"),
    ("Purple", "Púrpura"),
    ("Reconnect Full Resolution Media…", "Reconectar medios de resolución completa…"),
    ("Recover Unsaved Changes…", "Recuperar cambios no guardados…"),
    ("Rectangle", "Rectángulo"),
    ("Red", "Rojo"),
    ("Redo", "Rehacer"),
    ("Reference Monitor", "Monitor de referencia"),
    ("Remix", "Remezcla"),
    ("Remix Properties…", "Propiedades de remezcla…"),
    ("Remove Attributes…", "Quitar atributos…"),
    ("Remove Filler Words", "Quitar muletillas"),
    ("Remove Pauses", "Quitar pausas"),
    ("Remove Unused", "Quitar no utilizados"),
    ("Rename…", "Cambiar nombre…"),
    ("Render Audio", "Procesar audio"),
    ("Render Effects In to Out", "Procesar efectos de entrada a salida"),
    ("Render In to Out", "Procesar de entrada a salida"),
    ("Render Selection", "Procesar selección"),
    ("Replace Fonts in Projects…", "Reemplazar fuentes en proyectos…"),
    ("Replace With Clip", "Reemplazar con clip"),
    ("Report an Issue…", "Informar de un problema…"),
    ("Reset All Parameters", "Restablecer todos los parámetros"),
    ("Reset Duration", "Restablecer duración"),
    ("Reset to Saved Layout", "Restablecer al diseño guardado"),
    ("Restore Captions from Source Clip", "Restaurar subtítulos del clip de origen"),
    ("Reveal Log Files…", "Mostrar archivos de registro…"),
    ("Reverse Match Frame", "Fotograma coincidente inverso"),
    ("Revert", "Revertir"),
    ("Revert Remix", "Revertir remezcla"),
    ("Review", "Revisión"),
    ("Right", "Derecha"),
    ("Ripple Delete", "Eliminar y cerrar hueco"),
    ("Ripple Sequence Markers", "Desplazar marcadores de secuencia"),
    ("Rose", "Rosa"),
    ("Safe Margins", "Márgenes seguros"),
    ("Save", "Guardar"),
    ("Save All", "Guardar todo"),
    ("Save As…", "Guardar como…"),
    ("Save Guides as Template…", "Guardar guías como plantilla…"),
    ("Save a Copy…", "Guardar una copia…"),
    ("Save as Template…", "Guardar como plantilla…"),
    ("Scale to Frame Size", "Escalar al tamaño del fotograma"),
    ("Scene Edit Detection…", "Detección de cambios de escena…"),
    ("Scratch Disks…", "Discos de memoria virtual…"),
    ("Search Bin", "Bandeja de búsqueda"),
    ("Select", "Seleccionar"),
    ("Select All", "Seleccionar todo"),
    ("Select All Matching", "Seleccionar todos los coincidentes"),
    ("Select Label Group", "Seleccionar grupo de etiquetas"),
    ("Select Next Graphic", "Seleccionar siguiente gráfico"),
    ("Select Next Layer", "Seleccionar siguiente capa"),
    ("Select Previous Graphic", "Seleccionar gráfico anterior"),
    ("Select Previous Layer", "Seleccionar capa anterior"),
    ("Selection Follows Playhead", "La selección sigue al cabezal"),
    ("Selection as FilmCraft Project…", "Selección como proyecto de FilmCraft…"),
    ("Selection…", "Selección…"),
    ("Send Backward", "Enviar atrás"),
    ("Send to Back", "Enviar al fondo"),
    ("Sequence", "Secuencia"),
    ("Sequence From Clip", "Secuencia a partir de clip"),
    ("Sequence Settings…", "Ajustes de secuencia…"),
    ("Composite in Linear Color", "Componer en color lineal"),
    ("Import PSD as Sequence…", "Importar PSD como secuencia…"),
    ("Sequence…", "Secuencia…"),
    ("Show Active Caption Tracks Only", "Mostrar solo pistas de subtítulos activas"),
    ("Show All Caption Tracks", "Mostrar todas las pistas de subtítulos"),
    ("Show All Marker Colors", "Mostrar todos los colores de marcador"),
    ("Show Guides", "Mostrar guías"),
    ("Show Rulers", "Mostrar reglas"),
    ("Show Through Edits", "Mostrar ediciones pasantes"),
    ("Simplify Sequence…", "Simplificar secuencia…"),
    ("Snap in Program Monitor", "Ajustar en el monitor de programa"),
    ("Snap in Timeline", "Ajustar en la línea de tiempo"),
    ("Source", "Origen"),
    ("Source Settings…", "Ajustes de origen…"),
    ("Speed/Duration…", "Velocidad/duración…"),
    ("Synchronize…", "Sincronizar…"),
    ("System Compatibility Report…", "Informe de compatibilidad del sistema…"),
    ("Tan", "Canela"),
    ("Teal", "Verde azulado"),
    ("Text", "Texto"),
    ("Time Interpolation", "Interpolación de tiempo"),
    ("Timecode", "Código de tiempo"),
    ("Timecode…", "Código de tiempo…"),
    ("Timeline", "Línea de tiempo"),
    ("Timeline…", "Línea de tiempo…"),
    ("Toggle Proxies", "Alternar proxies"),
    ("Tools", "Herramientas"),
    ("Top", "Arriba"),
    ("Transcribe Sequence…", "Transcribir secuencia…"),
    ("Transcribe…", "Transcribir…"),
    ("Transcript", "Transcripción"),
    ("Transparent Video…", "Video transparente…"),
    ("Trim Edit", "Recortar edición"),
    ("Trim…", "Recorte…"),
    ("Undo", "Deshacer"),
    ("Ungroup", "Desagrupar"),
    ("Universal Counting Leader…", "Cuenta atrás universal…"),
    ("Update Metadata…", "Actualizar metadatos…"),
    ("Upgrade Caption to Graphic", "Convertir subtítulo en gráfico"),
    ("Upgrade to Source Graphic", "Convertir en gráfico de origen"),
    ("Vertical Text", "Texto vertical"),
    ("Video In", "Entrada de video"),
    ("Video Options", "Opciones de video"),
    ("Video Out", "Salida de video"),
    ("Video and Audio Waveform Split", "Video y forma de onda de audio divididos"),
    ("View", "Ver"),
    ("Violet", "Violeta"),
    ("Window", "Ventana"),
    ("Workspaces", "Espacios de trabajo"),
    ("Yellow", "Amarillo"),
    ("Zoom In", "Acercar"),
    ("Zoom Out", "Alejar"),
    ("Zoom to Sequence", "Ajustar zoom a la secuencia"),
];

const JAPANESE: &[(&str, &str)] = &[
    ("Clip", "クリップ"),
    ("Sequence", "シーケンス"),
    ("Markers", "マーカー"),
    ("Graphics and Titles", "グラフィックスとタイトル"),
    ("Settings", "環境設定"),
    ("Object", "オブジェクト"),
    ("Effect", "効果"),
    ("Settings…", "環境設定…"),
    ("Image", "画像"),
    ("Layer", "レイヤー"),
    ("Type", "書式"),
    ("Select", "選択"),
    ("Filter", "フィルター"),
    ("Window", "ウィンドウ"),
    ("Language", "表示言語"),
    ("Save As…", "別名で保存…"),
    ("Exit", "終了"),
    ("New…", "新規…"),
    ("New", "新規"),
    ("Horizontal", "横書き"),
    ("Vertical", "縦書き"),
    ("Orientation", "組み方向"),
    ("Layers", "レイヤー"),
    ("History", "履歴"),
    ("Properties", "プロパティ"),
    ("Color", "カラー"),
    ("Brush Settings", "ブラシ設定"),
    ("Tools", "ツール"),
    ("Options", "オプション"),
    ("Zoom In", "ズームイン"),
    ("Zoom Out", "ズームアウト"),
    ("Fit on Screen", "画面に合わせる"),
    ("Copy", "コピー"),
    ("Cut", "切り取り"),
    ("Paste", "貼り付け"),
    ("Select All", "すべて選択"),
    ("Deselect", "選択を解除"),
    ("Export", "書き出し"),
    ("Export As…", "形式を指定して書き出し…"),
    ("Search…", "検索…"),
    ("Theme", "テーマ"),
    ("Menu", "メニュー"),
    ("File", "ファイル"),
    ("Edit", "編集"),
    ("Pages", "ページ"),
    ("View", "表示"),
    ("Help", "ヘルプ"),
    ("Preferences", "環境設定"),
    ("Preferences…", "環境設定…"),
    ("Interface language", "表示言語"),
    ("Open…", "開く…"),
    ("New blank PDF", "空白の PDF を作成"),
    ("Create PDF from file…", "ファイルから PDF を作成…"),
    ("Create PDF from images…", "画像から PDF を作成…"),
    ("Create PDF from clipboard", "クリップボードから PDF を作成"),
    ("Combine files…", "ファイルを結合…"),
    ("Save", "保存"),
    ("Save as…", "別名で保存…"),
    ("Close file", "ファイルを閉じる"),
    ("Close all", "すべて閉じる"),
    ("Revert", "保存済みの状態に戻す"),
    ("Print…", "印刷…"),
    ("Document properties…", "文書のプロパティ…"),
    ("Undo", "取り消し"),
    ("Redo", "やり直し"),
    ("Find…", "検索…"),
    ("Advanced search…", "高度な検索…"),
    ("Copy pages", "ページをコピー"),
    ("Cut pages", "ページを切り取り"),
    ("Paste pages", "ページを貼り付け"),
    ("Fit visible", "表示範囲に合わせる"),
    ("Marquee zoom", "範囲指定ズーム"),
    ("Take a snapshot", "スナップショットを作成"),
    ("Full screen mode", "全画面表示"),
    ("Read mode", "閲覧モード"),
    ("Switch light / dark theme", "明るい／暗いテーマを切り替え"),
    ("Comments panel", "コメントパネル"),
    ("Form fields panel", "フォームフィールドパネル"),
    ("Clear form", "フォームをクリア"),
    ("Find tools and commands…", "ツールとコマンドを検索…"),
    ("Zoom", "ズーム"),
    ("Actual size", "実際のサイズ"),
    ("Zoom to page level", "ページ全体を表示"),
    ("Fit to width", "幅に合わせる"),
    ("Display theme", "表示テーマ"),
    ("Side panels", "サイドパネル"),
    ("OK", "OK"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translations_are_unique_and_preserve_unknown_text() {
        for (i, (en, ja)) in JAPANESE.iter().enumerate() {
            assert!(!ja.is_empty());
            assert!(JAPANESE.iter().take(i).all(|(other, _)| en != other));
            assert_eq!(Language::En.tr(en), *en);
        }
        assert_eq!(Language::Ja.tr("File"), "ファイル");
        for (i, (en, es)) in SPANISH.iter().enumerate() {
            assert!(!es.is_empty());
            assert!(SPANISH.iter().take(i).all(|(other, _)| en != other));
        }
        assert_eq!(Language::Es.tr("File"), "Archivo");
        assert_eq!(Language::Es.tr("mi video.mp4"), "mi video.mp4");
        assert_eq!(Language::parse("es"), Some(Language::Es));
        assert_eq!(Language::Ja.tr("日本語の文書.pdf"), "日本語の文書.pdf");
        assert_eq!(Language::parse("xx"), None);
    }

    #[test]
    fn spanish_entries_are_menu_labels() {
        // a renamed command would otherwise leave a dead entry and an untranslated menu item
        let app = crate::FilmcraftApp::new(filmcraft_engine::Session::default());
        let items = crate::menus::menu_items(&app);
        let known = |text: &str| crate::menus::MENUS.contains(&text) || items.iter().any(|it| it.label == text || it.path.iter().any(|p| p == text));
        for (en, _) in SPANISH {
            assert!(known(en), "not a menu label: {en}");
        }
    }

    #[test]
    fn language_commands_switch_and_persist_without_a_document() {
        let mut app = crate::FilmcraftApp::new(filmcraft_engine::Session::default());
        let ctx = egui::Context::default();
        crate::menus::invoke(&mut app, &ctx, "app.language.japanese", serde_json::json!({})).unwrap();
        assert_eq!(app.ui.language, Language::Ja);
        let saved = serde_json::to_string(&app.ui).unwrap();
        let restored: crate::state::UiState = serde_json::from_str(&saved).unwrap();
        assert_eq!(restored.language, Language::Ja);
        crate::menus::invoke(&mut app, &ctx, "app.language.spanish", serde_json::json!({})).unwrap();
        assert_eq!(app.ui.language, Language::Es);
        assert!(crate::menus::menu_items(&app).iter().any(|it| it.id == "app.language.spanish" && it.checked == Some(true)));
        crate::menus::invoke(&mut app, &ctx, "app.language.english", serde_json::json!({})).unwrap();
        assert_eq!(app.ui.language, Language::En);
    }

    #[test]
    fn japanese_needs_an_installed_font() {
        let mut app = crate::FilmcraftApp::new(filmcraft_engine::Session::default());
        let ctx = egui::Context::default();
        crate::theme::install(&ctx, &crate::theme::Tokens::for_kind(crate::theme::ThemeKind::default()));
        let r = crate::menus::invoke(&mut app, &ctx, "app.language.japanese", serde_json::json!({}));
        let craft = craft_japanese_font();
        if !craft && system_japanese_font().is_none() {
            // no craft-fonts and no system font: refused, and the interface stays English
            assert!(r.is_err(), "{r:?}");
            assert_eq!(app.ui.language, Language::En);
            return;
        }
        assert!(r.is_ok(), "{r:?}");
        let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
        output.textures_delta.clear();
        ctx.fonts_mut(|fonts| {
            // every theme family falls back to the craft-fonts (when built with them; no system
            // font is added then) or to the system Japanese font
            for family in crate::theme::font_families() {
                let stack = fonts.definitions().families.get(&family).cloned().unwrap_or_default();
                if craft {
                    assert!(stack.last().is_some_and(|n| n.starts_with("craft:")), "{family:?}: {stack:?}");
                    assert!(!stack.iter().any(|n| n == JAPANESE_FONT), "{family:?}: {stack:?}");
                } else {
                    assert_eq!(stack.last().map(String::as_str), Some(JAPANESE_FONT), "{family:?}: {stack:?}");
                }
            }
            // and the glyphs resolve. (Only families whose replacement-box face is another font:
            // egui's `has_glyph` reports false for any character served by the face it also uses
            // for the replacement box, which in the Inter-only "medium"/"semibold" stacks is the
            // Japanese font itself.)
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                let font = egui::FontId::new(13.0, family);
                for ch in JAPANESE_SAMPLE.chars() {
                    assert!(fonts.has_glyph(&font, ch), "missing {ch} in {font:?}");
                }
            }
        });
    }
}
