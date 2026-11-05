import { useMemo, useState } from "react";

interface Dir {
  dirs: Map<string, Dir>;
  files: string[];
}

function build(paths: string[]): Dir {
  const root: Dir = { dirs: new Map(), files: [] };
  for (const path of paths) {
    const parts = path.split("/");
    let node = root;
    for (const part of parts.slice(0, -1)) {
      if (!node.dirs.has(part)) node.dirs.set(part, { dirs: new Map(), files: [] });
      node = node.dirs.get(part)!;
    }
    node.files.push(path);
  }
  return root;
}

interface Props {
  paths: string[];
  active: string | null;
  dirty: Set<string>;
  /** Who else has each file open. */
  peers?: Map<string, { clientId: number; name: string; color: string }[]>;
  onOpen: (path: string) => void;
  onDelete?: (path: string) => void;
}

export function FileTree(props: Props) {
  const root = useMemo(() => build(props.paths), [props.paths]);
  return (
    <ul className="tree" role="tree">
      <DirNode dir={root} prefix="" depth={0} {...props} />
    </ul>
  );
}

function DirNode({ dir, prefix, depth, ...props }: Props & { dir: Dir; prefix: string; depth: number }) {
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const toggle = (name: string) =>
    setCollapsed((c) => {
      const next = new Set(c);
      if (next.has(name)) next.delete(name);
      else next.add(name);
      return next;
    });
  const pad = { paddingLeft: 8 + depth * 12 };
  return (
    <>
      {[...dir.dirs.entries()]
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([name, sub]) => (
          <li key={prefix + name} role="treeitem" aria-expanded={!collapsed.has(name)}>
            <button className="tree-row" style={pad} onClick={() => toggle(name)}>
              <span className="tree-icon">{collapsed.has(name) ? "▸" : "▾"}</span>
              {name}
            </button>
            {!collapsed.has(name) && (
              <ul role="group">
                <DirNode dir={sub} prefix={`${prefix}${name}/`} depth={depth + 1} {...props} />
              </ul>
            )}
          </li>
        ))}
      {dir.files.map((path) => (
        <li key={path} role="treeitem" className="tree-file">
          <button
            className={`tree-row ${path === props.active ? "active" : ""}`}
            style={pad}
            onClick={() => props.onOpen(path)}
          >
            <span className="tree-icon" />
            {path.split("/").pop()}
            {props.dirty.has(path) && <span className="dot">●</span>}
            {props.peers?.get(path)?.map((p) => (
              <span key={p.clientId} className="peer-dot" style={{ background: p.color }} title={`${p.name} is here`} />
            ))}
          </button>
          {props.onDelete && (
            <button className="tree-delete" aria-label={`Delete ${path}`} title="Delete" onClick={() => props.onDelete!(path)}>
              ×
            </button>
          )}
        </li>
      ))}
    </>
  );
}
