# Music Magic Nix library - tags and path schema helpers
rec {
  # Path-relevant tags for use in pathSchema
  tags = builtins.listToAttrs (map (name: {
    inherit name;
    value = { __type = "pathSchemaTag"; inherit name; };
  }) [
    "ARTIST" "ALBUMARTIST" "ALBUM" "TITLE"
    "TRACKNUMBER" "DISCNUMBER" "DATE" "GENRE"
    "LABEL" "CATALOGNUMBER"
  ]);

  # Check if a value is a tag reference
  isTag = x: builtins.isAttrs x && x.__type or null == "pathSchemaTag";

  # Serialize a single segment part (tag or literal string)
  serializePart = part:
    if isTag part then "\${${part.name}}"
    else if builtins.isString part then part
    else throw "pathSchema part must be a tag or string, got: ${builtins.typeOf part}";

  # Serialize a path schema segment (single tag, string, or list of parts)
  serializeSegment = seg:
    if isTag seg then "\${${seg.name}}"
    else if builtins.isString seg then seg
    else if builtins.isList seg then builtins.concatStringsSep "" (map serializePart seg)
    else throw "pathSchema segment must be a tag, string, or list, got: ${builtins.typeOf seg}";

  # Serialize full path schema (list of segments joined by /)
  serializePathSchema = schema:
    if schema == null then null
    else if builtins.isList schema then builtins.concatStringsSep "/" (map serializeSegment schema)
    else if builtins.isString schema then schema  # Allow legacy string format
    else throw "pathSchema must be a list or string, got: ${builtins.typeOf schema}";
}
