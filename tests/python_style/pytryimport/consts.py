def setup():
    try:
        from .universe import register_universe
        return register_universe("wufu", [1, 2, 3])
    except ImportError:
        return 0
