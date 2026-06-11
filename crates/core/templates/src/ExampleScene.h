#ifndef XENOLITH_PROJECT_EXAMPLESCENE_H_
#define XENOLITH_PROJECT_EXAMPLESCENE_H_

#include "XL2dScene.h"
#include "XL2dLabel.h"

namespace STAPPLER_VERSIONIZED stappler::xenolith::app {

// A minimal scene: an empty window with a single centered label.
class ExampleScene : public basic2d::Scene2d {
public:
	virtual ~ExampleScene() = default;

	virtual bool init(NotNull<AppThread>, NotNull<AppWindow>,
			const core::FrameConstraints &) override;
	virtual void handleContentSizeDirty() override;

protected:
	using Scene::init;
	basic2d::Label *_label = nullptr;
};

} // namespace stappler::xenolith::app

#endif /* XENOLITH_PROJECT_EXAMPLESCENE_H_ */
