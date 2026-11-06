#include "FramePass.h"

#include "Shaders/FramePass.generated.h"

#include <guesthouse_display_protocol.h>

#include <algorithm>
#include <cstring>

using Microsoft::WRL::ComPtr;

namespace Guesthouse
{

namespace
{

constexpr UINT GroupSize = 8; // numthreads in the shader.
constexpr UINT CursorSide = 256; // The largest cursor zako's own export takes.
constexpr UINT CursorBytesPerPixel = 4;
constexpr UINT WordBytes = 4;

UINT GroupsFor(UINT pixels)
{
	return (pixels + GroupSize - 1) / GroupSize;
}

}

FramePass::FramePass(ID3D11Device* device, ID3D11DeviceContext* context) : m_Device(device), m_Context(context)
{
}

HRESULT FramePass::Init()
{
	HRESULT hr = m_Device->CreateComputeShader(g_FramePassShader, sizeof(g_FramePassShader), nullptr, &m_Shader);
	if (FAILED(hr))
	{
		return hr;
	}

	D3D11_BUFFER_DESC constants = {};
	constants.ByteWidth = sizeof(PassConstants);
	constants.Usage = D3D11_USAGE_DYNAMIC;
	constants.BindFlags = D3D11_BIND_CONSTANT_BUFFER;
	constants.CPUAccessFlags = D3D11_CPU_ACCESS_WRITE;
	hr = m_Device->CreateBuffer(&constants, nullptr, &m_Constants);
	if (FAILED(hr))
	{
		return hr;
	}

	hr = CreateLutBuffer();
	return SUCCEEDED(hr) ? CreateCursorTexture() : hr;
}

HRESULT FramePass::CreateLutBuffer()
{
	D3D11_BUFFER_DESC desc = {};
	desc.ByteWidth = GammaLutEntries * sizeof(float) * 4;
	desc.Usage = D3D11_USAGE_DEFAULT;
	desc.BindFlags = D3D11_BIND_SHADER_RESOURCE;
	HRESULT hr = m_Device->CreateBuffer(&desc, nullptr, &m_Lut);
	if (FAILED(hr))
	{
		return hr;
	}

	D3D11_SHADER_RESOURCE_VIEW_DESC view = {};
	view.Format = DXGI_FORMAT_R32G32B32A32_FLOAT;
	view.ViewDimension = D3D11_SRV_DIMENSION_BUFFER;
	view.Buffer.NumElements = GammaLutEntries;
	return m_Device->CreateShaderResourceView(m_Lut.Get(), &view, &m_LutView);
}

HRESULT FramePass::CreateCursorTexture()
{
	D3D11_TEXTURE2D_DESC desc = {};
	desc.Width = CursorSide;
	desc.Height = CursorSide;
	desc.MipLevels = 1;
	desc.ArraySize = 1;
	desc.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
	desc.SampleDesc.Count = 1;
	desc.Usage = D3D11_USAGE_DEFAULT;
	desc.BindFlags = D3D11_BIND_SHADER_RESOURCE;
	HRESULT hr = m_Device->CreateTexture2D(&desc, nullptr, &m_Cursor);
	return SUCCEEDED(hr) ? m_Device->CreateShaderResourceView(m_Cursor.Get(), nullptr, &m_CursorView) : hr;
}

HRESULT FramePass::CreateRing(std::uint32_t slots, std::uint32_t width, std::uint32_t height, std::uint32_t generation)
{
	DestroyRing();
	const std::uint64_t size = static_cast<std::uint64_t>(PitchFor(width)) * height;

	D3D11_BUFFER_DESC desc = {};
	desc.ByteWidth = static_cast<UINT>(size);
	desc.Usage = D3D11_USAGE_DEFAULT;
	desc.BindFlags = D3D11_BIND_UNORDERED_ACCESS;
	desc.MiscFlags = D3D11_RESOURCE_MISC_BUFFER_ALLOW_RAW_VIEWS;

	D3D11_UNORDERED_ACCESS_VIEW_DESC view = {};
	view.Format = DXGI_FORMAT_R32_TYPELESS;
	view.ViewDimension = D3D11_UAV_DIMENSION_BUFFER;
	view.Buffer.NumElements = static_cast<UINT>(size / WordBytes);
	view.Buffer.Flags = D3D11_BUFFER_UAV_FLAG_RAW;

	for (std::uint32_t slot = 0; slot < slots; ++slot)
	{
		ComPtr<ID3D11Buffer> buffer;
		ComPtr<ID3D11UnorderedAccessView> access;
		HRESULT hr = m_Device->CreateBuffer(&desc, nullptr, &buffer);
		if (SUCCEEDED(hr))
		{
			hr = m_Device->CreateUnorderedAccessView(buffer.Get(), &view, &access);
		}
		if (FAILED(hr))
		{
			DestroyRing();
			return hr;
		}

		const gh_slot_stamp stamp = {GH_STAMP_MAGIC, generation, slot};
		const D3D11_BOX box = {0, 0, 0, sizeof(stamp), 1, 1};
		m_Context->UpdateSubresource(buffer.Get(), 0, &box, &stamp, 0, 0);

		m_Slots.push_back(buffer);
		m_SlotViews.push_back(access);
	}

	m_SlotSize = size;
	m_Context->Flush();
	return S_OK;
}

void FramePass::DestroyRing()
{
	m_SlotViews.clear();
	m_Slots.clear();
	m_SlotSize = 0;
}

HRESULT FramePass::SetLut(const GammaTransform& transform)
{
	if (!transform.lutEnabled || transform.lut.size() != GammaLutEntries)
	{
		return S_OK;
	}
	m_Context->UpdateSubresource(m_Lut.Get(), 0, nullptr, transform.lut.data(), 0, 0);
	return S_OK;
}

HRESULT FramePass::SetCursorShape(const CursorImage::Snapshot& cursor)
{
	if (cursor.pixels.empty() || cursor.width > CursorSide || cursor.height > CursorSide ||
	    cursor.pitch < cursor.width * CursorBytesPerPixel)
	{
		return E_INVALIDARG;
	}
	const D3D11_BOX box = {0, 0, 0, cursor.width, cursor.height, 1};
	m_Context->UpdateSubresource(m_Cursor.Get(), 0, &box, cursor.pixels.data(), cursor.pitch, 0);
	return S_OK;
}

// Nothing in Microsoft's docs says the surface can be bound to a shader. Zako only ever copies it. If it can't be, one
// CopyResource into a texture of our own makes it readable, at the cost of one more pass over the frame.
HRESULT FramePass::ReadableView(ID3D11Texture2D* surface, ComPtr<ID3D11ShaderResourceView>& view)
{
	D3D11_TEXTURE2D_DESC desc = {};
	surface->GetDesc(&desc);
	if (desc.BindFlags & D3D11_BIND_SHADER_RESOURCE)
	{
		return m_Device->CreateShaderResourceView(surface, nullptr, &view);
	}

	if (!m_Bounce || m_BounceDesc.Width != desc.Width || m_BounceDesc.Height != desc.Height || m_BounceDesc.Format != desc.Format)
	{
		D3D11_TEXTURE2D_DESC bounce = desc;
		bounce.MipLevels = 1;
		bounce.ArraySize = 1;
		bounce.SampleDesc.Count = 1;
		bounce.SampleDesc.Quality = 0;
		bounce.Usage = D3D11_USAGE_DEFAULT;
		bounce.BindFlags = D3D11_BIND_SHADER_RESOURCE;
		bounce.CPUAccessFlags = 0;
		bounce.MiscFlags = 0;
		m_Bounce.Reset();
		HRESULT hr = m_Device->CreateTexture2D(&bounce, nullptr, &m_Bounce);
		if (FAILED(hr))
		{
			return hr;
		}
		m_BounceDesc = bounce;
	}
	m_Context->CopyResource(m_Bounce.Get(), surface);
	return m_Device->CreateShaderResourceView(m_Bounce.Get(), nullptr, &view);
}

HRESULT FramePass::Run(ID3D11Texture2D* surface, std::uint32_t slot, const PassConstants& constants)
{
	if (slot >= m_Slots.size())
	{
		return E_INVALIDARG;
	}

	ComPtr<ID3D11ShaderResourceView> source;
	HRESULT hr = ReadableView(surface, source);
	if (FAILED(hr))
	{
		return hr;
	}

	D3D11_MAPPED_SUBRESOURCE mapped = {};
	hr = m_Context->Map(m_Constants.Get(), 0, D3D11_MAP_WRITE_DISCARD, 0, &mapped);
	if (FAILED(hr))
	{
		return hr;
	}
	std::memcpy(mapped.pData, &constants, sizeof(constants));
	m_Context->Unmap(m_Constants.Get(), 0);

	ID3D11ShaderResourceView* views[] = {source.Get(), m_CursorView.Get(), m_LutView.Get()};
	ID3D11UnorderedAccessView* target = m_SlotViews[slot].Get();
	ID3D11Buffer* buffers[] = {m_Constants.Get()};

	m_Context->CSSetShader(m_Shader.Get(), nullptr, 0);
	m_Context->CSSetConstantBuffers(0, 1, buffers);
	m_Context->CSSetShaderResources(0, 3, views);
	m_Context->CSSetUnorderedAccessViews(0, 1, &target, nullptr);
	m_Context->Dispatch(GroupsFor(constants.width), GroupsFor(constants.height), 1);

	// Unbinding keeps Windows' surface free of any hold of ours once it goes back.
	ID3D11ShaderResourceView* noViews[] = {nullptr, nullptr, nullptr};
	ID3D11UnorderedAccessView* noTarget = nullptr;
	m_Context->CSSetShaderResources(0, 3, noViews);
	m_Context->CSSetUnorderedAccessViews(0, 1, &noTarget, nullptr);

	m_Context->Flush();
	return S_OK;
}

}
