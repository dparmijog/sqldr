# ADR 0001: heurística UTF-8 para columnas `BLOB`/`VARBINARY`/`BINARY`

## Contexto

`sqldr-core::mysql::decode_value` decide cómo convertir una celda MySQL a
`Value` a partir del nombre de tipo que expone `sqlx` (`MySqlTypeInfo::name`).
Ese nombre ya distingue de forma determinista texto de binario en el caso
general: `sqlx` deriva `"TEXT"/"TINYTEXT"/"MEDIUMTEXT"/"LONGTEXT"` cuando el
flag `ColumnFlags::BINARY` de la definición de columna está apagado, y
`"BLOB"/"TINYBLOB"/"MEDIUMBLOB"/"LONGBLOB"/"VARBINARY"/"BINARY"` cuando está
prendido (ver `sqlx-mysql-0.8.6/src/protocol/text/column.rs::ColumnType::name`).

El problema: ese flag no significa "esto es binario" de forma confiable.
MySQL lo usa también para columnas de texto declaradas con collation binaria
(`_bin` o charset `binary`). Ejemplo real encontrado durante la verificación
del Hito 1: `information_schema.COLUMNS.COLUMN_TYPE` y `.COLUMN_KEY` llegan
con el flag `BINARY` activo aunque su contenido es texto legible
(`"int"`, `"varchar(50)"`, `"PRI"`, …). Decodificarlas siempre como
`Value::Bytes` las muestra como hex ilegible en la tabla de resultados.

## Decisión

En la rama `BLOB`/`VARBINARY`/`BINARY` de `decode_value`, se intenta
decodificar los bytes como UTF-8 primero; si es válido, se guarda como
`Value::Text`. Solo si la conversión falla se conserva como `Value::Bytes`
(mostrado en hex).

## Consecuencias

- Beneficio: catálogos de sistema y columnas de texto con collation binaria
  se muestran legibles, que es el caso más común en uso interactivo.
- Costo aceptado: una secuencia de bytes genuinamente binaria puede, por
  coincidencia, ser UTF-8 válida (p. ej. `0xDEAD` es un carácter Unicode
  válido de 2 bytes) y mostrarse como texto en vez de hex. No existe una
  señal estática 100% correcta: el flag `BINARY` de MySQL no distingue
  "texto con collation binaria" de "binario real".
- Si en el futuro esto genera confusión real con datos binarios grandes
  (imágenes, blobs de aplicación), la mitigación más simple es un umbral de
  tamaño (columnas grandes se muestran siempre en hex/truncadas) en vez de
  intentar refinar la heurística por tipo.
