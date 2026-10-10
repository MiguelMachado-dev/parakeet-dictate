from ..overlay_frame import Renderer
from . import bars, fluid

STYLES: dict[str, Renderer] = {"fluid": fluid.render, "bars": bars.render}
DEFAULT_STYLE = "fluid"
