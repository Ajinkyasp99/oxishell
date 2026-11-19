# Input is the array of release pages collected by jq --slurp.
[
  .[][]
  | select(.draft == false)
  | .assets[]
  | select(.name | endswith(".deb") or endswith(".dmg"))
  | .download_count
]
| {schemaVersion: 1, label: "package downloads", message: ((add // 0) | tostring), color: "blue"}
