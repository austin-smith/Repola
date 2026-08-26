export interface GitErrorGuidance {
  title: string;
  guidance: string[];
  category: "authentication" | "hook" | "lfs" | "safeDirectory" | "transport" | "generic";
}

export function classifyGitError(message: string): GitErrorGuidance {
  const value = message.toLowerCase();
  if (includesAny(value, ["dubious ownership", "safe.directory", "unsafe repository"])) {
    return {
      category: "safeDirectory",
      title: "Git does not trust this repository directory",
      guidance: [
        "Verify that the repository is owned by the expected account on the machine where it lives.",
        "If the ownership is intentional, add only this exact repository path to Git’s safe.directory configuration, then retry.",
      ],
    };
  }
  if (includesAny(value, ["git-lfs", "git lfs", "filter-process", "smudge filter lfs", "lfs objects", "batch response:"])) {
    return {
      category: "lfs",
      title: "Git LFS needs attention",
      guidance: [
        "Confirm Git LFS is installed and available to Git on this machine.",
        "Check LFS authentication and storage quota, then run the repository’s LFS fetch or pull before retrying.",
      ],
    };
  }
  if (includesAny(value, ["hook declined", "pre-commit hook", "commit-msg hook", "pre-push hook", "hooks/", "hook failed"])) {
    return {
      category: "hook",
      title: "A repository hook stopped Git",
      guidance: [
        "Read the hook output below; hooks commonly report a failing check or required formatting command.",
        "Fix the reported issue and retry. Repola does not bypass repository hooks.",
      ],
    };
  }
  if (includesAny(value, ["authentication failed", "permission denied (publickey", "could not read username", "could not read password", "terminal prompts disabled", "invalid username or password", "repository not found", "access denied", "http 401", "http 403"])) {
    return {
      category: "authentication",
      title: "Git could not authenticate to the remote",
      guidance: [
        value.includes("publickey")
          ? "Verify the SSH key, host alias, and agent available to Git on this machine. This is separate from Repola’s machine connection."
          : "Refresh the credential stored by your Git credential helper or provider CLI on this machine.",
        "Confirm the remote URL and that the authenticated account can access this repository.",
      ],
    };
  }
  if (includesAny(value, ["could not resolve host", "connection refused", "connection reset", "connection timed out", "operation timed out", "network is unreachable", "ssl certificate", "tls", "ssh_exchange_identification", "connection closed by remote host", "broken pipe"])) {
    return {
      category: "transport",
      title: "The Git transport failed",
      guidance: [
        "Check DNS, VPN, proxy, firewall, and remote-host availability from the machine where this repository lives.",
        "Retry after connectivity is restored. The complete transport diagnostic is preserved below.",
      ],
    };
  }
  return {
    category: "generic",
    title: "Git could not complete the operation",
    guidance: ["Review the complete Git diagnostic below, correct the reported condition, and retry."],
  };
}

function includesAny(value: string, candidates: string[]): boolean {
  return candidates.some((candidate) => value.includes(candidate));
}
