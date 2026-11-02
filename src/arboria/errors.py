"""Exception types shown to the user as clean one-line errors."""


class ArbError(Exception):
    """An expected failure; the CLI prints the message and exits with status 1."""


class NotARepoError(ArbError):
    """The working directory is not inside a git repository."""


class NotInitializedError(ArbError):
    """The repository has no .arboria/ state directory yet."""
