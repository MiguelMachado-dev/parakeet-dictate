<#
.SYNOPSIS
Downloads what the executable loads at run time: the ONNX Runtime and CUDA libraries, and Parakeet Ultra.

.DESCRIPTION
runtime\ gets the 13 DLLs the app loads, taken from pinned wheels on PyPI, about 1 GB to download and
1.3 GB on disk. models\parakeet-ultra gets the model at a pinned revision, about 3.2 GB. Every download is
checked against its SHA-256. Rerunning skips what is already in place and resumes a partial download.

-Root points the script at another folder, so a test leaves the real files alone.

.EXAMPLE
.\scripts\fetch_assets.ps1
#>
param(
    [string]$Root = (Join-Path $PSScriptRoot "..")
)

$ErrorActionPreference = "Stop"
$root = (Resolve-Path $Root).Path
$runtime = Join-Path $root "runtime"
$model = Join-Path $root "models\parakeet-ultra"
$cache = Join-Path $root "runtime\.downloads"

$pypi = "https://files.pythonhosted.org/packages"
$wheels = @(
    @{
        Url  = "$pypi/7e/ff/035730b399eea860df5952a76cc489cb34a663e5341eb6f53d892a5f7f2c/onnxruntime_gpu-1.31.0-cp312-cp312-win_amd64.whl"
        Hash = "e6bd756a3022a1a5a0217f35814e40e6e04fd1e497a5be78f991f6bfefad2fba"
        Dlls = "onnxruntime", "onnxruntime_providers_cuda", "onnxruntime_providers_shared"
    },
    @{
        Url  = "$pypi/a3/df/f1246959833e2c437db8be3e5b477f66b87f8817821ed40de6c7561c9a36/nvidia_cublas-13.8.0.4-py3-none-win_amd64.whl"
        Hash = "8c5494423bb8a46822cb6b0cb95d7fa4be2d7b96a31155dff083839ec8297910"
        Dlls = "cublas64_13", "cublasLt64_13"
    },
    @{
        Url  = "$pypi/87/6a/e55ff0ac26a5c6e2b21f41c9d04ad096b4ed6da593fba7e25845c61b0532/nvidia_cudnn_cu13-9.27.0.42-py3-none-win_amd64.whl"
        Hash = "7d96f634adafd55c72231eb0500ca77ab109ec8ebff7b33000b76e081bc4558e"
        Dlls = "cudnn64_9", "cudnn_adv64_9", "cudnn_engines_precompiled64_9", "cudnn_engines_runtime_compiled64_9",
            "cudnn_engines_tensor_ir64_9", "cudnn_graph64_9", "cudnn_heuristic64_9", "cudnn_ops64_9"
    }
)

$modelBase = "https://huggingface.co/Olicorne/parakeet-tdt-0.6b-v3-ultra-onnx/resolve/b27f93c2238aa35e138996c005a62f1e8430f801"
# The folder reads flat; the repo keeps each precision in its own.
$modelFiles = [ordered]@{
    "config.json"                        = "666903c76b9798caf2c210afd4f6cd60b08a8dbf9800ec8d7a3bc0d2148ac466"
    "vocab.txt"                          = "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d"
    "nemo128.onnx"                       = "a9fde1486ebfcc08f328d75ad4610c67835fea58c73ba57e3209a6f6cf019e9f"
    "fp32/decoder_joint-model.onnx"      = "7fa05c5183dfc89dbb0f34d4b2e7935f877762f2438bbcc79729f4e2d00eb041"
    "fp32/decoder_joint-model.onnx.data" = "5d6873a9bf49b704270a3e3dcc6c71090e1edd3248c6499b5d3cadd9f23dbcec"
    "fp32/encoder-model.onnx"            = "af8d232c654c696eb07a3cf9fa164a9649209b759409e6ec9913f8155d255a23"
    "fp32/encoder-model.onnx.data.000"   = "2762ecdbc59be479843dca0140e49300fd6e98ec994e19d0a7003308f0b3282d"
    "fp32/encoder-model.onnx.data.001"   = "d26ce092d3291e893f79794cb06306f6d18390dee332330d68fc4b096b4762f3"
    "int8/decoder_joint-model.int8.onnx" = "667ffde2242a560923cba6bc7f69ef03d736f8a36ca9588f2e6f9fa818cf2fc1"
    "int8/encoder-model.int8.onnx"       = "714047d70be4b612dce02e0ab2f9e0d6552f78adc421f408d154070796bc2dcf"
}

function Get-Verified([string]$url, [string]$hash, [string]$path) {
    $part = "$path.part"
    & curl.exe --location --fail --retry 3 --continue-at - --silent --show-error --output $part $url
    if ($LASTEXITCODE -ne 0) {
        throw "Could not download $url (curl exit code $LASTEXITCODE)."
    }
    if ((Get-FileHash -Algorithm SHA256 -LiteralPath $part).Hash -ne $hash) {
        Remove-Item -LiteralPath $part
        throw "The download of $url does not match its SHA-256; removed it. Run this again."
    }
    Move-Item -LiteralPath $part -Destination $path -Force
}

New-Item -ItemType Directory -Force $runtime, $model, $cache | Out-Null
Add-Type -AssemblyName System.IO.Compression.FileSystem

foreach ($wheel in $wheels) {
    $missing = @($wheel.Dlls | Where-Object { -not (Test-Path -LiteralPath (Join-Path $runtime "$_.dll")) })
    if ($missing.Count -eq 0) {
        continue
    }
    $name = ($wheel.Url -split "/")[-1]
    $path = Join-Path $cache $name
    "Downloading $name"
    Get-Verified $wheel.Url $wheel.Hash $path
    $zip = [IO.Compression.ZipFile]::OpenRead($path)
    try {
        foreach ($entry in $zip.Entries) {
            if ($entry.Name -notlike "*.dll" -or [IO.Path]::GetFileNameWithoutExtension($entry.Name) -notin $missing) {
                continue
            }
            $target = Join-Path $runtime $entry.Name
            [IO.Compression.ZipFileExtensions]::ExtractToFile($entry, "$target.part", $true)
            Move-Item -LiteralPath "$target.part" -Destination $target -Force
        }
    } finally {
        $zip.Dispose()
    }
    Remove-Item -LiteralPath $path
    foreach ($dll in $missing) {
        if (-not (Test-Path -LiteralPath (Join-Path $runtime "$dll.dll"))) {
            throw "$name holds no $dll.dll."
        }
    }
}
Remove-Item -LiteralPath $cache -Recurse -Force

foreach ($file in $modelFiles.GetEnumerator()) {
    $target = Join-Path $model (Split-Path -Leaf $file.Key)
    if (-not (Test-Path -LiteralPath $target)) {
        "Downloading $($file.Key)"
        Get-Verified "$modelBase/$($file.Key)" $file.Value $target
    }
}
"Runtime in $runtime, model in $model."
