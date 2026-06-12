#include "XLCommon.h"
#include "XL2dSceneContent.h"
#include "XLEntryPoint.h"
#include "ExampleScene.h"

namespace STAPPLER_VERSIONIZED stappler::xenolith::app {

bool ExampleScene::init(NotNull<AppThread> app, NotNull<core::RenderServerChannel> window,
		const core::FrameConstraints &constraints) {
	using namespace basic2d;

	if (!Scene2d::init(app, window, constraints)) {
		return false;
	}

	auto content = Rc<SceneContent2d>::create();

	// The whole scene: one centered label.
	_label = content->addChild(Rc<Label>::create("Hello from Xenolith!"), ZOrder(1));
	_label->setAnchorPoint(Anchor::Middle);
	_label->setFontSize(32);

	setContent(content);
	return true;
}

void ExampleScene::handleContentSizeDirty() {
	Scene2d::handleContentSizeDirty();
	auto cs = getContentSize();
	if (_label) {
		_label->setPosition(Vec2(cs.width / 2.0f, cs.height / 2.0f));
	}
}

// Registers ExampleScene as the application's primary scene class.
DEFINE_PRIMARY_SCENE_CLASS(ExampleScene)

} // namespace stappler::xenolith::app
