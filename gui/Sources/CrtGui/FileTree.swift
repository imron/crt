import Foundation

struct FileTreeNode: Identifiable, Hashable {
    let id: String
    let name: String
    let file: GuiFile?
    let children: [FileTreeNode]?

    var isFile: Bool { file != nil }

    static func forest(from files: [GuiFile]) -> [FileTreeNode] {
        class Dir {
            var subdirs: [String: Dir] = [:]
            var files: [GuiFile] = []
        }

        let root = Dir()
        for file in files {
            let parts = file.path.split(separator: "/").map(String.init)
            guard !parts.isEmpty else {
                continue
            }
            var dir = root
            for component in parts.dropLast() {
                if let existing = dir.subdirs[component] {
                    dir = existing
                } else {
                    let child = Dir()
                    dir.subdirs[component] = child
                    dir = child
                }
            }
            dir.files.append(file)
        }

        func build(_ dir: Dir, path: String) -> [FileTreeNode] {
            let directoryNodes = dir.subdirs.keys.sorted {
                $0.localizedStandardCompare($1) == .orderedAscending
            }
            .compactMap { name -> FileTreeNode? in
                guard let child = dir.subdirs[name] else {
                    return nil
                }
                let childPath = path.isEmpty ? name : "\(path)/\(name)"
                return FileTreeNode(
                    id: "dir:\(childPath)",
                    name: name,
                    file: nil,
                    children: build(child, path: childPath)
                )
            }

            let fileNodes = dir.files
                .sorted { $0.path.localizedStandardCompare($1.path) == .orderedAscending }
                .map { file in
                    FileTreeNode(
                        id: file.path,
                        name: file.path.split(separator: "/").last.map(String.init) ?? file.path,
                        file: file,
                        children: nil
                    )
                }

            return directoryNodes + fileNodes
        }

        return build(root, path: "")
    }
}
