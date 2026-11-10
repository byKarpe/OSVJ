# OSVJ — OpenSourceVJ (v1.2)

Motor de visuales y proyección en tiempo real escrito en Rust con aceleración por hardware WGPU (DirectX 12 / Vulkan).

## Características principales
- Arquitectura Multi-Deck e independencia de capas sin cortes en vivo.
- Diseñador de Cortes (Slices) con ajuste de 4 esquinas, Modo Malla (3x3 / 4x4) y corrección automática de aspecto.
- Decodificador On-Demand (.MOV DXV3) y conversor integrado de .MP4 a .MOV.
- Enrutamiento Multi-Monitor en Windows y sistema de Auto-BackUp cada 5 minutos.

## Compilación
Ejecutar en terminal: cargo run --release --bin OSVJ
