import Darwin

/// Access control lists beyond the POSIX bits, as Info shows them:
/// "R ACL" when an entry concerns reading, "W ACL" when one concerns writing.
enum ACLInfo {
    static func summary(atPath path: String) -> String? {
        guard let acl = acl_get_link_np(path, ACL_TYPE_EXTENDED) else { return nil }
        defer { acl_free(UnsafeMutableRawPointer(acl)) }
        var reads = false
        var writes = false
        var entry: acl_entry_t?
        var which = ACL_FIRST_ENTRY.rawValue
        while acl_get_entry(acl, Int32(which), &entry) == 0, let current = entry {
            which = ACL_NEXT_ENTRY.rawValue
            var permset: acl_permset_t?
            guard acl_get_permset(current, &permset) == 0, let permset else { continue }
            let read = [ACL_READ_DATA, ACL_READ_ATTRIBUTES, ACL_READ_EXTATTRIBUTES, ACL_READ_SECURITY]
            let write = [
                ACL_WRITE_DATA, ACL_APPEND_DATA, ACL_DELETE, ACL_DELETE_CHILD, ACL_WRITE_ATTRIBUTES,
                ACL_WRITE_EXTATTRIBUTES, ACL_WRITE_SECURITY, ACL_CHANGE_OWNER,
            ]
            if read.contains(where: { acl_get_perm_np(permset, $0) == 1 }) { reads = true }
            if write.contains(where: { acl_get_perm_np(permset, $0) == 1 }) { writes = true }
        }
        let parts = (reads ? ["R ACL"] : []) + (writes ? ["W ACL"] : [])
        return parts.isEmpty ? nil : parts.joined(separator: ", ")
    }
}
