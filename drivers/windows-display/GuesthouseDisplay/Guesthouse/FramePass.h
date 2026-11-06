#pragma once

#include "ColourTransform.h"
#include "CursorImage.h"

#include <d3d11.h>
#include <wrl/client.h>

#include <cstdint>
#include <vector>

namespace Guesthouse
{

// Keep in step with the cbuffer in Shaders/FramePass.hlsl.
struct PassConstants
{
	std::uint32_t width;
	std::uint32_t height;
	std::uint32_t pitch;
	std::uint32_t source;

	std::uint32_t wire;
	std::uint32_t lutEnabled;
	std::uint32_t cursorEnabled;
	float sdrWhite;

	std::int32_t cursorX;
	std::int32_t cursorY;
	std::uint32_t cursorWidth;
	std::uint32_t cursorHeight;

	float sourceToWire[3][4];
	float cursorToWire[3][4];
};

static_assert(sizeof(PassConstants) % 16 == 0, "D3D11 constant buffers come in 16-byte units");

// The GPU side of the exporter: the ring's buffers, and the compute pass that fills them. It runs on the swap chain's thread only.
class FramePass
{
public:
	FramePass(ID3D11Device* device, ID3D11DeviceContext* context);

	HRESULT Init();

	// Makes the slots, each stamped so that the daemon can tell them apart. Flushes, so that the stamps are on their way
	// before REGISTER is sent.
	HRESULT CreateRing(std::uint32_t slots, std::uint32_t width, std::uint32_t height, std::uint32_t generation);
	void DestroyRing();

	std::uint64_t SlotSize() const { return m_SlotSize; }

	// Takes a new transform's lookup table to the GPU.
	HRESULT SetLut(const GammaTransform& transform);
	// Takes a new cursor shape to the GPU.
	HRESULT SetCursorShape(const CursorImage::Snapshot& cursor);

	// Writes one frame into a slot and flushes. The surface is free for Windows once this returns.
	HRESULT Run(ID3D11Texture2D* surface, std::uint32_t slot, const PassConstants& constants);

private:
	HRESULT CreateCursorTexture();
	HRESULT CreateLutBuffer();
	HRESULT ReadableView(ID3D11Texture2D* surface, Microsoft::WRL::ComPtr<ID3D11ShaderResourceView>& view);

	Microsoft::WRL::ComPtr<ID3D11Device> m_Device;
	Microsoft::WRL::ComPtr<ID3D11DeviceContext> m_Context;
	Microsoft::WRL::ComPtr<ID3D11ComputeShader> m_Shader;
	Microsoft::WRL::ComPtr<ID3D11Buffer> m_Constants;
	Microsoft::WRL::ComPtr<ID3D11Buffer> m_Lut;
	Microsoft::WRL::ComPtr<ID3D11ShaderResourceView> m_LutView;
	Microsoft::WRL::ComPtr<ID3D11Texture2D> m_Cursor;
	Microsoft::WRL::ComPtr<ID3D11ShaderResourceView> m_CursorView;

	// Only used if Windows hands us a surface a shader can't bind. See ReadableView.
	Microsoft::WRL::ComPtr<ID3D11Texture2D> m_Bounce;
	D3D11_TEXTURE2D_DESC m_BounceDesc = {};

	std::vector<Microsoft::WRL::ComPtr<ID3D11Buffer>> m_Slots;
	std::vector<Microsoft::WRL::ComPtr<ID3D11UnorderedAccessView>> m_SlotViews;
	std::uint64_t m_SlotSize = 0;
};

}
