"""Cluster backends: FakeSlurm (local, for tests and dry runs) and SshSlurm (real)."""

from __future__ import annotations

from ..clusters import Profile
from .base import Backend, BackendError, NeedLogin, map_slurm_state, restrict_path

__all__ = ["Backend", "BackendError", "NeedLogin", "get_backend", "map_slurm_state",
           "restrict_path"]


def get_backend(profile: Profile) -> Backend:
    if profile.kind == "fake":
        from .fake import FakeSlurm

        return FakeSlurm(profile)
    from .ssh import SshSlurm

    return SshSlurm(profile)
