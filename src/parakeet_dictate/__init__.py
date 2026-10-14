import os

# numpy's OpenBLAS starts a thread per core when it loads and commits a buffer for each, about 600 MB in
# all. The app's numpy work is small arrays.
os.environ.setdefault("OPENBLAS_NUM_THREADS", "1")
