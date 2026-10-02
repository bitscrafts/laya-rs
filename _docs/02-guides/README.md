# 02 — Guides

How to stand the decision layer up and wire it into an agent. Written for an operator
or agent who has not read the source; every command is real and every number was
measured.

| Guide | What it answers |
|---|---|
| [USING-LAYA-DAILY.md](USING-LAYA-DAILY.md) | **What do I run every day, what does it cost, and what does each failure mean?** The operator's guide: the real command surface, the token measurement, and the failure modes. |
| [RUNNING-LAYA.md](RUNNING-LAYA.md) | How do I run Laya + llama.cpp? Why are there two processes? How do I verify a real decision? |
| [INSTALL-IN-HERMES.md](INSTALL-IN-HERMES.md) | How do I register it in Hermes, and why does a new session matter after a rebuild? |

Supervision (keeping the head alive across crashes and reboots) is operational, not a
guide, and lives in [../05-operations/SUPERVISION.md](../05-operations/SUPERVISION.md).
